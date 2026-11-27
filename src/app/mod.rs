use crate::afford::{self, Clickable};
use crate::claude_opts::ClaudeOpts;
use crate::clip;
use crate::gitinfo::{GitInfo, GitWatcher};
use crate::gitops::{self, Change, DiffWatcher};
use crate::history::{self, HistoryItem};
use crate::hooks::{self, ClaudeStatus, HookMsg, HookServer, IpcCmd};
use crate::keys;
use crate::usage::{self, fmt_tokens, Usage, UsageWatcher};
use crate::layout::{Dir, Node};
use crate::platform::notify;
use crate::pathfmt;
use crate::prompts::{self, Snippet};
use crate::search::{self, SearchState};
use crate::style::{self, Icon as SIcon};
use crate::templates::{self, Template, TplItem};
use crate::platform::tray::{Tray, TrayCmd};
use crate::shells::{self, ShellSpec};
use crate::state::{self, Lang, Profile, Project, SavedState, SessionKind, SessionMeta, Settings};
use crate::chat::{self as chatmod, ChatBackend, ChatState};
use crate::terminal::{CmdDone, Session, SpawnOptions};
use crate::termview;
use crate::theme::{self, Theme};
use eframe::egui::{
    self, Align2, Color32, CornerRadius, Event, FontId, Id, Key, Modifiers, Pos2, Rect, RichText, Sense,
    Stroke, StrokeKind, Ui, UiBuilder, Vec2,
};
use eframe::egui::Margin;
use style::{S1, S2, S3};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Kalender (#164): Ansichten, Dialoge; Einstieg `calendar_ui::show` (für den Zeit-Reiter).
pub(crate) mod calendar_ui;
mod access_ui;
mod chat_plan_ui;
mod chat_images;
mod chat_images_ui;
mod chat_ask_ui;
mod chat_ui;
mod cleanup_ui;
mod claude_status;
mod data;
mod dialogs;
mod dragdrop;
pub(crate) mod editor_ui;
mod empty_hub;
mod env_ui;
use env_ui::env_locked_text;
// Editor-Reiter (#54): Einstellungen, MCP-Werkzeuge, Plattenänderungen, --edit, Explorer-Eintrag, Suche in Dateien.
mod editor_services;
mod editor_search_ui;
pub(crate) mod folder_picker;
mod fresh_start;
mod inbox_ui;
mod closing;
mod conv_pick;
#[cfg(test)]
mod glyph_tests;
mod mcp;
mod models_ui;
mod multiwin;
mod prompt_ui;
mod palette_ui;
mod osc52;
mod link_ask;
mod list_edit;
mod opts_tabs;
mod vt_notify;
mod panels;
mod paste_ask;
mod planning_ui;
mod procs_ui;
mod save_actions_dialog;
mod save_actions_ui;
mod session_settings;
mod settings;
#[cfg(feature = "readme-shots")]
mod shot_start;
#[cfg(unix)]
mod secret_ui;
mod sidebar;
mod sysprompt_ui;
mod taint;
mod tasks_ui;
mod terminal_links;
mod backdrop;
mod theme_editor;
mod theme_preview;
// Reiter „Zeit“ (#164): Wecker, Timer, Kalender-Platz, Stoppuhren, Weltuhren, Geburtstage.
mod time_ui;
mod titlebar;
mod toasts;
mod tpl_lib;
mod tutorial;
pub(crate) mod update;

use tutorial::{Spot, TutMark};
use save_actions_dialog::SaPaletteCmd;

/// Vorgabefarbe einer neuen Sitzung/eines neuen Projekts Nummer `i` aus der geltenden Palette des Terminal-Hubs
/// (läuft im Kreis; bis 2.2 die feste Palette, jetzt `crate::palette::HUB_DEFAULTS` bzw. die gemeinsame).
fn hub_color(i: u64) -> [u8; 3] {
    crate::palette::nth_rgb(crate::palette::Area::Hub, i as usize)
}

struct Entry {
    meta: SessionMeta,
    live: Option<Session>,
    error: Option<String>,
    status: ClaudeStatus,
    status_msg: String,
    /// Ereignis in einer nicht angesehenen Sitzung (Markierung in der Seitenleiste).
    attention: bool,
    /// Aktueller Ordner laut Claude-Hooks (sonst Startordner).
    live_cwd: String,
    /// Erster gemeldeter Ordner (wo die Shell gestartet ist): Wurzel des Dateibaums, wenn der eingestellte
    /// Arbeitsordner nicht als Windows-Pfad bekannt ist (WSL ohne Ordner bzw. mit `~`, #277).
    start_cwd: String,
    /// Es kamen Hook-Ereignisse (sonst Status ggf. aus dem Transkript abgeleitet).
    hook_seen: bool,
    /// Die Warteschlange hat zuletzt automatisch gesendet.
    queue_active: bool,
    /// Seit wann die Freigabe-Frage angezeigt wird (Schutz gegen Fehlklicks auf eine gerade wechselnde Frage).
    perm_since: Option<std::time::Instant>,
    /// Für diese Sitzung wurde der Budget-Hinweis schon gezeigt.
    budget_warned: bool,
    /// Aufgelöste Shell-Kennung beim Start (z. B. `wsl:Ubuntu` auch bei Profilen mit eigenem Programm).
    shell_eff: String,
    /// Shell-Integration, mit der die Sitzung gestartet wurde (#57; zsh: PROMPT_EOL_MARK in `run_command` entfernen).
    shell_flavor: Option<crate::shellinit::Flavor>,
    /// Chat-Sitzung (`meta.kind = chat`): Verlauf, Eingabe, laufende Antwort. Hängt an der Sitzung, nicht am Fenster.
    chat: Option<ChatState>,
    /// Beim letzten Start lief Claude (`spawn_into`). `meta.claude` ist die Einstellung für den nächsten Start (#170);
    /// alles zur Laufzeit (Status, Warteschlange, Hooks, MCP) richtet sich nach diesem Wert.
    launched_claude: bool,
    /// Claude-Optionen dieses Starts (für Auskünfte über MCP; `meta.claude_opts` gilt erst beim nächsten Start).
    launched_opts: crate::claude_opts::ClaudeOpts,
    /// Geheimer Schlüssel des laufenden Prozesses (`PANELDECK_SESSION_KEY`, #152): belegt die Sitzung als Aufrufer, wenn
    /// der Prozessbaum nicht hilft (WSL). Je Start neu, nie angezeigt, protokolliert oder gespeichert.
    session_key: String,
    /// Konversation, die zu diesem Start gehört (#260): Meldungen mit einer anderen Konversations-ID (z. B. ein von
    /// Claude gestartetes `claude -p`, das die Umgebung erbt) zählen nicht. Je Start neu.
    hook_conv: hooks::ConvBinding,
    /// In diesem Start kam schon eine Meldung mit Schlüssel: Meldungen ohne Schlüssel (globale Hooks einer älteren
    /// paneldeck.exe) zählen dann nicht mehr.
    hook_keyed: bool,
    /// Hinweis vom letzten Start (z. B. weggelassene Pfade aus den Claude-Optionen, #182); wird im nächsten Bild als
    /// Meldung gezeigt.
    launch_note: Option<String>,
    /// Fortschritt laut Programm (OSC 9;4, #183) für Seitenleiste, Taskleiste und `list_sessions`.
    progress: Option<crate::vtnotify::Progress>,
    /// Stand des Claude dieses Starts über die Hook-Meldungen hinaus (laufende Runde, letzte Meldung, offene Rückfrage,
    /// Prompt der Shell) – für den Rückfall, wenn Meldungen ausbleiben (`claude_status`). Je Start neu.
    track: claude_status::Track,
    /// Die Sitzung wartet vor ihrem Start auf die Auswahl der Konversation (#302, `conv_pick`).
    pick_pending: bool,
    /// Geheime Umgebungsvariablen ließen sich beim Start nicht entschlüsseln (#384): die Sitzung wartet auf
    /// „Entsperren und starten“ bzw. neue Werte.
    env_locked: Option<crate::session_env::Locked>,
    /// Geänderte Umgebungsvariablen gelten erst nach einem Neustart (#384, oranger Punkt in der Kopfzeile).
    env_pending: bool,
    /// Variablen, die die laufende Shell hat: beim Start festgehalten, nach „Jetzt setzen“ fortgeschrieben (#384).
    env_live: Vec<crate::session_env::EnvVar>,
}

impl Entry {
    fn new(meta: SessionMeta) -> Self {
        let launched_claude = meta.claude && meta.kind != SessionKind::Chat;
        let launched_opts = if launched_claude { meta.claude_opts_eff().0 } else { Default::default() };
        Entry { meta, live: None, error: None, status: ClaudeStatus::Unknown, status_msg: String::new(), attention: false, live_cwd: String::new(), start_cwd: String::new(), hook_seen: false, queue_active: false, perm_since: None, budget_warned: false, shell_eff: String::new(), shell_flavor: None, chat: None, session_key: String::new(), hook_conv: Default::default(), hook_keyed: false, launched_claude, launched_opts, launch_note: None, progress: None, track: Default::default(), pick_pending: false, env_locked: None, env_pending: false, env_live: Vec::new() }
    }
    /// Chat-Sitzung mit geladenem Verlauf (kein Terminal, keine PTY).
    fn new_chat(meta: SessionMeta, chat: ChatState) -> Self {
        let mut e = Entry::new(meta);
        e.status = ClaudeStatus::Idle;
        e.chat = Some(chat);
        e
    }
    /// Läuft in dieser Sitzung Claude (so wie sie zuletzt gestartet wurde)? Nicht `meta.claude`: das gilt erst beim
    /// nächsten Start.
    fn is_claude(&self) -> bool {
        self.launched_claude && !self.is_chat()
    }
    /// Ein Chat statt eines Terminals (hat nie eine PTY; „beendet“ gibt es dort nicht).
    fn is_chat(&self) -> bool {
        self.meta.kind == SessionKind::Chat
    }
    /// Läuft etwas, das beim Schließen verloren ginge? Terminal: Prozess läuft; Chat: eine Antwort wird gerade erzeugt
    /// oder Werkzeuge laufen bzw. warten auf Freigabe.
    fn running(&self) -> bool {
        match &self.chat {
            Some(c) => c.busy(),
            None => !self.exited(),
        }
    }
    /// Chat mit mindestens einer Nachricht (Schließen löscht diesen Verlauf).
    fn chat_has_history(&self) -> bool {
        self.chat.as_ref().is_some_and(|c| !c.messages().is_empty())
    }
    /// Modellname eines Chats (leer bei Terminals).
    fn chat_model(&self) -> &str {
        self.meta.chat.as_ref().map(|c| c.model.as_str()).unwrap_or("")
    }
    /// Text in die Sitzung einfügen: Terminal bekommt ihn als Eingabe, ein Chat in sein Eingabefeld (nie abgeschickt).
    /// `false`, wenn es kein Ziel gibt (Terminal ohne Prozess).
    fn insert_text(&mut self, text: &str, submit: bool) -> bool {
        if let Some(c) = self.chat.as_mut() {
            if !c.draft.is_empty() && !c.draft.ends_with([' ', '\n']) {
                c.draft.push(' ');
            }
            c.draft += text;
            c.focus_input = true;
            return true;
        }
        match &self.live {
            Some(s) => {
                s.send_text(text, submit);
                true
            }
            None => false,
        }
    }
    /// Nächsten wartenden Prompt senden, wenn Claude von beschäftigt auf bereit wechselt.
    /// Gibt `true` zurück, wenn die Warteschlange gerade vollständig abgearbeitet wurde.
    fn queue_tick(&mut self, old: ClaudeStatus, new: ClaudeStatus) -> bool {
        if !self.is_claude() || self.meta.queue_paused || self.exited() {
            return false;
        }
        if new == ClaudeStatus::Idle && old != ClaudeStatus::Idle {
            if !self.meta.queue.is_empty() {
                self.send_next_queued();
            } else if self.queue_active {
                self.queue_active = false;
                return true;
            }
        }
        false
    }

    fn send_next_queued(&mut self) {
        if self.meta.queue.is_empty() {
            return;
        }
        let text = self.meta.queue.remove(0);
        match &self.live {
            // In eine beendete Shell geht nichts mehr: der Prompt bleibt in der Warteschlange.
            Some(s) if !s.has_exited() => {
                s.send_text(&text, true);
                self.assume_working();
                self.queue_active = true;
            }
            _ => self.meta.queue.insert(0, text),
        }
    }

    fn eff_shell(&self) -> &str {
        if self.shell_eff.is_empty() { &self.meta.shell_id } else { &self.shell_eff }
    }

    fn cwd_now(&self) -> String {
        if !self.live_cwd.is_empty() {
            self.live_cwd.clone()
        } else if self.meta.cwd.is_empty() && self.eff_shell().starts_with("wsl") && !self.is_chat() {
            // WSL ohne Ordner startet im Linux-Home (#172).
            "~".to_string()
        } else if self.meta.cwd.is_empty() {
            home_dir()
        } else {
            self.meta.cwd.clone()
        }
    }
    fn status_label(&self, lang: Lang) -> &'static str {
        if let Some(c) = &self.chat {
            // Fragen des Modells (#356) warten auf den Nutzer – wie eine Freigabe, aber mit eigenem Text.
            return if c.awaiting_answer() {
                lang.t("wartet auf dich", "waiting for you")
            } else if c.status() == chatmod::ChatStatus::Approval {
                lang.t("Freigabe?", "approval?")
            } else if c.streaming() {
                lang.t("schreibt", "streaming")
            } else if c.busy() {
                lang.t("Werkzeug", "tool")
            } else if c.paused {
                lang.t("angehalten", "paused")
            } else if c.has_error() {
                lang.t("Fehler", "error")
            } else {
                lang.t("bereit", "idle")
            };
        }
        if self.exited() {
            return "";
        }
        match self.status {
            ClaudeStatus::Unknown => "",
            ClaudeStatus::Idle => lang.t("bereit", "ready"),
            ClaudeStatus::Working => lang.t("arbeitet", "working"),
            ClaudeStatus::Permission => lang.t("Freigabe?", "approval?"),
            ClaudeStatus::Input => lang.t("Eingabe", "input"),
        }
    }
    fn needs_you(&self) -> bool {
        !self.exited() && matches!(self.status, ClaudeStatus::Permission | ClaudeStatus::Input)
    }
    fn status_color(&self, t: f64) -> Color32 {
        if self.chat.as_ref().is_some_and(|c| c.has_error()) {
            return crate::style::ui().error;
        }
        if self.exited() {
            return Color32::from_gray(110);
        }
        match self.status {
            ClaudeStatus::Unknown | ClaudeStatus::Idle => crate::style::ui().success,
            ClaudeStatus::Working => {
                let k = 0.55 + 0.45 * ((t * 4.0).sin() as f32 * 0.5 + 0.5);
                crate::style::brand().gamma_multiply(k)
            }
            ClaudeStatus::Permission => crate::style::ui().danger,
            ClaudeStatus::Input => crate::style::ui().warning,
        }
    }
}

impl Entry {
    fn display_name(&self) -> String {
        if self.meta.custom_name {
            return self.meta.name.clone();
        }
        self.live
            .as_ref()
            .and_then(|s| s.title())
            .unwrap_or_else(|| self.meta.name.clone())
    }
    fn color(&self) -> Color32 {
        let c = self.meta.color;
        Color32::from_rgb(c[0], c[1], c[2])
    }
    /// Terminal ohne laufenden Prozess. Ein Chat ist nie „beendet“ (siehe `running`).
    fn exited(&self) -> bool {
        if self.is_chat() {
            return false;
        }
        self.live.as_ref().map(|s| s.has_exited()).unwrap_or(true)
    }
}

#[derive(Clone, Copy)]
enum Nav {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone)]
enum Action {
    /// Formatier-Befehl des Editors auf den aktiven Reiter (#381, Befehls-Palette).
    EditorFormat(crate::editor::format::Op),
    /// Befehl des Editors aus der Befehlspalette (#385).
    Editor(editor_ui::EditorAction),
    /// Eingefügter Text mit mehreren Zeilen wartet auf die Rückfrage (Sitzung, Text, aus abgedocktem Fenster).
    /// Sitzung, Text, aus abgedocktem Fenster, Rückfrage nur wegen möglicher Broadcast-Ziele (`Session::take_paste_ask`).
    PasteAsk(u64, String, bool, bool),
    OpenPalette,
    /// Aktionen beim Speichern (#383) aus der Befehls-Palette.
    SaveActions(SaPaletteCmd),
    OpenTemplates,
    OpenCosts,
    TemplateStart(usize),
    /// Vorlage starten, mit Ziel (Ordner, Bereich; #244).
    TemplateStartAt(usize, tpl_lib::TplAt),
    /// Vorlage löschen – nach der Rückfrage; nur, wenn an Index noch dieser Name steht (#276).
    TemplateDelete(usize, String),
    /// Rückfrage vor dem Löschen einer Vorlage (#276).
    TemplateDeleteAsk(usize),
    TemplateSave(String, Option<String>),
    /// Editor der Bibliothek: Vorlage bearbeiten / neue Terminal-Vorlage / duplizieren / aus einer Sitzung (#244).
    TemplateEdit(usize),
    TemplateNew,
    TemplateDuplicate(usize),
    TemplateFromSession(u64),
    ResumeItem(history::HistoryItem),
    ToggleTree,
    FolderNew(String),
    FolderRename(String),
    /// Ordner (Projekt, Pfad) löschen, Inhalt rückt hoch – nach der Rückfrage von `FolderDeleteAsk` (#276).
    FolderDelete(u64, String),
    /// Kontextmenü „Ordner löschen (Inhalt bleibt)“: Rückfrage, ein leerer Ordner verschwindet sofort (#276).
    FolderDeleteAsk(String),
    /// Ordner (Projekt, Pfad) samt Unterordnern entfernen – nach dem Schließen seiner Sitzungen (#276).
    FolderRemove(u64, String),
    FolderToggle(String),
    FolderColor(String, Option<[u8; 3]>),
    FolderCwd(String, bool),
    FolderProfile(String, u64),
    FolderNewSession(String, bool),
    FolderRestartAll(String),
    /// Rückfrage „alle schließen“ (Ordner, Projekt, alle) öffnen.
    CloseAll(closing::CloseScope),
    /// Bestätigt: diese Sitzungen beenden.
    CloseMany(Vec<u64>),
    FolderLayout(String),
    FolderBroadcast(String),
    MoveFolder(String, String),
    FolderTemplate(String),
    ToggleComposer,
    SetProjectProfile(u64, u64),
    OpenQueue(u64),
    ToggleDiff,
    QueueAdd(u64, String),
    /// Freigabe-Frage von Claude beantworten: Sitzung, Ja/Nein, laufende Nummer des gezeigten Aufrufs (#390).
    Approve(u64, bool, u64),
    OpenGlobalSearch,
    GlobalJump(u64, i32, String),
    JumpCmd(i32),
    ExportHistory(u64),
    OpenEditor,
    Cycle(i32),
    Detach(u64),
    Attach(u64),
    SwitchProject(u64),
    NewProjectPrompt,
    ProjectPrompt(u64, PromptKind),
    SetProjectColor(u64, [u8; 3]),
    DeleteProject(u64),
    /// Rückfrage vor dem Löschen eines Projekts (#276).
    DeleteProjectAsk(u64),
    TagsPrompt(u64),
    ToggleFiles,
    InsertPath(String),
    NewSessionAt(String),
    OpenHistory,
    ResumeHistory(usize),
    OpenSearch,
    OpenSettings,
    /// Fenster „Aufräumen …“ (#307).
    OpenCleanup,
    OpenPrompts,
    OpenNotes(u64),
    /// Agenten-Kette einer Sitzung aufheben (nur der Nutzer, Kontextmenü, #152).
    ResetAgentChain(u64),
    /// Mini-Suche „Ticket verknüpfen“ für die Sitzung öffnen.
    LinkTicket(u64),
    /// Ticket öffnen (Chip, Palette).
    OpenTicket(i64),
    /// Verknüpfung zwischen Sitzung und Ticket entfernen.
    UnlinkTicket(u64, i64),
    NewDialog(bool),
    ToggleBroadcast,
    SendSnippet(usize),
    FocusDir(Nav),
    Activate(u64),
    StartRename(u64),
    SetColor(u64, [u8; 3]),
    SetGroup(u64, String),
    Restart(u64),
    /// Wartet die Sitzung auf die Auswahl der Konversation (#302): ohne Auswahl neu starten.
    ConvPickFresh(u64),
    /// Hauptfenster (mit der Auswahl der Konversation) nach vorn holen, auch aus dem Tray (#302).
    ConvPickShow,
    /// „Neu laden“ (#169): Rückfrage, solange ein Prozess läuft, dann `Restart`. Chats: nichts.
    RequestReload(u64),
    /// Dialog „Einstellungen der Sitzung“ (#170) öffnen.
    OpenSessionSettings(u64),
    Duplicate(u64),
    RequestClose(u64),
    Close(u64),
    /// „Alle beenden“ (#168): alle Prozesse im Job dieser Sitzungen beenden (vor `Close`/`CloseMany`/`Restart`).
    /// (Sitzung, Job beim Öffnen der Rückfrage) – ein inzwischen neu gestarteter Job bleibt (#213).
    EndJobs(Vec<(u64, u64)>),
    Split(u64, Dir),
    SplitAt(u64, Dir, bool),
    /// Bereich minimieren: aus dem Layout nehmen, Sitzung läuft weiter (`App::minimize_pane`).
    MinimizePane(u64),
    Focus(u64),
    /// Sitzung für einen Agenten zeigen (`focus_session`/`create_session` mit `placement`, #162): Sitzung, Platzierung,
    /// aufrufende Sitzung des Agenten (deren Bereich ist der aktive Bereich, wenn er zu sehen ist), geschützte Sitzungen (nie
    /// verdrängen: Freigabe-Frage offen, Kette des Agenten). Bewegt nie den Tastaturfokus des Nutzers und holt kein Fenster
    /// nach vorn (`multiwin::brings_to_front`).
    Place(u64, crate::layout::Placement, Option<u64>, Vec<u64>),
    /// Zur Sitzung springen (aus einem Ticket): Terminal Hub ihres Fensters zeigen und sie aktivieren.
    JumpTo(u64),
    NewQuick,
    /// Neuer Chat im aktiven Projekt (öffnet den Dialog „Neuer Chat“).
    NewChat,
    /// Neuer Chat in diesem Ordner.
    FolderNewChat(String),
    /// Eingabe des Chats senden (oder die bearbeitete letzte Nachricht übernehmen).
    ChatSend(u64),
    /// Laufende Antwort abbrechen.
    ChatStop(u64),
    /// Letzte Antwort neu erzeugen (auch „Wiederholen“ nach einem Fehler).
    ChatRegenerate(u64),
    /// Letzte eigene Nachricht zum Bearbeiten ins Eingabefeld holen.
    ChatEdit(u64),
    /// Entscheidung zum wartenden Werkzeugaufruf (Chat, Aufruf-Kennung, Erlauben / Immer für diesen Chat / Ablehnen).
    ChatDecide(u64, String, chatmod::Decision),
    /// „Weiter“ nach dem Anhalten (Höchstzahl der Werkzeugaufrufe, Kontext voll).
    ChatContinue(u64),
    /// Dauerhafte Freigabe des Chats zurücknehmen.
    ChatRevoke(u64, chatmod::AllowRule),
    /// Antwort auf die einmalige Frage nach der Stufe des Modells.
    ChatLevel(u64, crate::mcp::McpLevel),
    /// Knopf der Plan-Karte (#358): Ausführen, Änderungen anfragen, Verwerfen, Exportieren.
    ChatPlan(u64, chatmod::plan::PlanAction),
    /// Chat jetzt kompaktieren (#359, Knopf „Kompaktieren“).
    ChatCompact(u64),
    /// Bild in den Chat (#357): Strg+V oder Büroklammer.
    ChatImage(u64, chat_images_ui::ImageReq),
    /// Tutorial (#143): Tour starten, fortsetzen, „Was ist neu“, Hinweis ausblenden.
    Tutorial(tutorial::Cmd),
    /// Umgebungsvariablen (#384): kopieren, einfügen, als .env kopieren, gesperrte entsperren und starten, bearbeiten.
    EnvCopy(u64),
    EnvPaste(u64),
    EnvCopyDotenv(u64),
    EnvUnlock(u64),
    EnvEdit(u64),
}

struct NewDialog {
    shell_id: String,
    cwd: String,
    name: String,
    claude: bool,
    /// Claude-Startoptionen (Modell, Effort, Permission-Mode, Agent); vorbelegt mit der letzten Wahl bzw. den Standards.
    copts: ClaudeOpts,
    /// Wie die Konversation beginnt und ob sie beim Neu laden/Neustart fortgesetzt wird (#170, eine Auswahl seit #241).
    conv: session_settings::ConvChoice,
    /// Beim ersten Bild den Fokus ins Namensfeld (nicht ins Terminal dahinter, #255).
    focus: bool,
    profile: u64,
    prompt: String,
    worktree: bool,
    branch: String,
    repo_for: String,
    repo: Option<PathBuf>,
    group: String,
    /// Ordnerfeld (WSL-fähig, #172); `cwd` wird jedes Bild daraus übernommen.
    picker: folder_picker::FolderPicker,
    /// Eigene Farbe (#292); aus = Farbe des Profils bzw. automatisch.
    own_color: bool,
    color: [u8; 3],
    /// Tags, mit Komma getrennt (Vorschläge aus den vorhandenen Sitzungen wie in den Einstellungen der Sitzung, #239).
    tags: String,
    notes: String,
    /// Vorhandene Tags für die Vorschläge (setzt die App beim Öffnen).
    tag_values: Vec<planning_ui::value_suggest::Counted>,
    /// Reiter „Umgebung“ (#384).
    env: env_ui::EnvEditor,
}

#[derive(Default)]
struct NewOpts {
    shell_id: Option<String>,
    cwd: Option<String>,
    name: Option<String>,
    claude: bool,
    claude_args: String,
    /// Claude-Startoptionen; `None` = Standards aus den Einstellungen (ohne bypassPermissions).
    claude_opts: Option<ClaudeOpts>,
    /// Nur der Dialog „Neue Sitzung“: der Nutzer hat bypassPermissions ausdrücklich für diese Sitzung gewählt.
    allow_bypass: bool,
    continue_last: bool,
    /// „Fortsetzen nach Neustart“ (#302; None = fortsetzen, wie bisher).
    resume_mode: Option<crate::state::ResumeMode>,
    prompt: Option<String>,
    resume_id: Option<String>,
    transcript: String,
    /// None = Standardprofil des Projekts, Some(0) = kein Profil
    profile: Option<u64>,
    group: Option<String>,
    /// Sitzung nur anlegen, nicht im Fenster aktivieren (Agenten-Schnittstelle: kein Fokusklau).
    quiet: bool,
    /// Fenster der Sitzung (#110); None = das gerade gezeichnete Fenster. Für Anlagen, die erst später im Hauptbild
    /// fertig werden (Worktree) oder von dort aus für ein anderes Fenster laufen (MCP, abgedockte Bereiche).
    window: Option<u64>,
    /// Projekt der Sitzung; None = aktives Projekt ihres Fensters.
    project: Option<u64>,
    /// Chat statt Terminal: mit diesem Server/Modell (kein Prozess, Verlauf in `chats/<id>.json`).
    chat: Option<state::ChatMeta>,
    /// Eigene Farbe (Dialog „Neue Sitzung“, #292); None = Farbe des Profils bzw. automatisch.
    color: Option<[u8; 3]>,
    tags: Vec<String>,
    notes: String,
    /// Umgebungsvariablen der Sitzung (#384; aus Dialog, Vorlage, Duplikat oder MCP `env`).
    env: Vec<crate::session_env::EnvVar>,
}

/// Farbe einer neuen Sitzung: eigene (Dialog „Neue Sitzung“, #292), sonst die des Profils, sonst automatisch.
fn session_color(own: Option<[u8; 3]>, profile: Option<[u8; 3]>, auto: [u8; 3]) -> [u8; 3] {
    own.or(profile).unwrap_or(auto)
}

struct Launch<'a> {
    restore: bool,
    restart: bool,
    continue_last: bool,
    prompt: Option<&'a str>,
    resume_id: Option<&'a str>,
}

impl NewDialog {
    fn new(shell_id: String, claude: bool, copts: ClaudeOpts) -> Self {
        let cwd = home_dir();
        let picker = folder_picker::FolderPicker::new("newdlg", &cwd, folder_picker::Target::Windows);
        NewDialog {
            shell_id,
            cwd,
            name: String::new(),
            claude,
            copts,
            conv: session_settings::ConvChoice::Resume,
            focus: true,
            profile: 0,
            prompt: String::new(),
            worktree: false,
            branch: String::new(),
            repo_for: String::new(),
            repo: None,
            group: String::new(),
            picker,
            own_color: false,
            color: crate::palette::nth_rgb(crate::palette::Area::Hub, 0),
            tags: String::new(),
            notes: String::new(),
            tag_values: Vec::new(),
            env: Default::default(),
        }
    }

    /// Startordner von außen setzen (Profil, Ordnervorgabe).
    fn set_cwd(&mut self, c: &str) {
        self.picker.set_text(c);
        self.cwd = self.picker.value();
    }
}

/// Eingabedialog für Ordner: 0 = neuer Unterordner, 1 = umbenennen, 2 = Prompt an alle Sitzungen senden.
struct FolderPrompt {
    kind: u8,
    path: String,
    text: String,
}

struct Prompt {
    id: u64,
    text: String,
}

#[derive(Clone, Copy, PartialEq)]
enum PromptKind {
    NewProject,
    RenameProject,
    ProjectFolder,
    /// Arbeitsordner eines Ordners im Baum (`TextPrompt::folder`), nur bei WSL-Shell als Dialog (#172).
    FolderCwd,
}

struct TextPrompt {
    kind: PromptKind,
    id: u64,
    tags_for: Option<u64>,
    text: String,
    /// Ordner im Baum (`PromptKind::FolderCwd`).
    folder: String,
    /// Ordnerfeld statt Textfeld (Ordnervorgaben, deren Shell WSL ist, #172).
    picker: Option<folder_picker::FolderPicker>,
}

struct EditorFile {
    label: String,
    path: PathBuf,
}

struct Editor {
    files: Vec<EditorFile>,
    sel: usize,
    text: String,
    orig: String,
    /// Entwürfe nicht angezeigter Dateien: Index → (Text, Original, nur lesbar)
    drafts: HashMap<usize, (String, String, bool)>,
    msg: String,
    msg_ok: bool,
    /// Datei ist nicht als UTF-8 lesbar: nur anzeigen, nicht überschreiben.
    readonly: bool,
}

impl Editor {
    fn new(project_dir: &str) -> Self {
        let home = hooks::global_settings_path().and_then(|p| p.parent().map(|d| d.to_path_buf()));
        let proj = PathBuf::from(project_dir);
        let mut files = vec![
            EditorFile { label: format!("CLAUDE.md ({})", state::t("Projekt", "project")), path: proj.join("CLAUDE.md") },
            EditorFile { label: format!(".claude/settings.json ({})", state::t("Projekt", "project")), path: proj.join(".claude").join("settings.json") },
            EditorFile { label: format!(".claude/settings.local.json ({})", state::t("Projekt", "project")), path: proj.join(".claude").join("settings.local.json") },
        ];
        if let Some(h) = home {
            files.push(EditorFile { label: format!("~/.claude/CLAUDE.md ({})", state::t("global", "global")), path: h.join("CLAUDE.md") });
            files.push(EditorFile { label: format!("~/.claude/settings.json ({})", state::t("global", "global")), path: h.join("settings.json") });
        }
        let mut e = Editor { files, sel: 0, text: String::new(), orig: String::new(), drafts: HashMap::new(), msg: String::new(), msg_ok: true, readonly: false };
        e.load();
        e
    }

    fn load(&mut self) {
        self.readonly = false;
        match std::fs::read(&self.files[self.sel].path) {
            Ok(b) => match String::from_utf8(b) {
                Ok(t) => self.text = t,
                Err(e) => {
                    // Keine stille Datenverfälschung: verlustbehaftet anzeigen, aber Speichern sperren.
                    self.text = String::from_utf8_lossy(e.as_bytes()).to_string();
                    self.readonly = true;
                    self.msg = state::t("Datei ist nicht UTF-8 (z. B. Latin-1) – nur Ansicht, Speichern ist gesperrt, um sie nicht zu beschädigen.", "File is not UTF-8 (e.g. Latin-1) – view only, saving is disabled so it is not damaged.").into();
                    self.msg_ok = false;
                }
            },
            Err(_) => self.text.clear(),
        }
        self.orig = self.text.clone();
    }

    fn select(&mut self, i: usize) {
        if i == self.sel {
            return;
        }
        self.drafts.insert(self.sel, (std::mem::take(&mut self.text), std::mem::take(&mut self.orig), self.readonly));
        self.sel = i;
        match self.drafts.remove(&i) {
            Some((t, o, ro)) => {
                self.text = t;
                self.orig = o;
                self.readonly = ro;
            }
            None => self.load(),
        }
        self.msg.clear();
    }

    fn dirty(&self) -> bool {
        self.text != self.orig
    }

    fn save(&mut self) {
        if self.readonly {
            self.msg = state::t("Speichern gesperrt: Datei ist nicht UTF-8.", "Saving disabled: file is not UTF-8.").into();
            self.msg_ok = false;
            return;
        }
        let path = self.files[self.sel].path.clone();
        if path.extension().is_some_and(|e| e == "json") && !self.text.trim().is_empty() {
            if let Err(e) = serde_json::from_str::<serde_json::Value>(&self.text) {
                self.msg = format!("{}: {e}", state::t("Ungültiges JSON", "Invalid JSON"));
                self.msg_ok = false;
                return;
            }
        }
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        match crate::state::write_atomic(&path, self.text.as_bytes()) {
            Ok(()) => {
                self.orig = self.text.clone();
                self.msg = state::t("Gespeichert", "Saved").into();
                self.msg_ok = true;
            }
            Err(e) => {
                self.msg = format!("{}: {e}", state::t("Fehler beim Speichern", "Error while saving"));
                self.msg_ok = false;
            }
        }
    }
}

#[derive(Default)]
struct GlobalSearch {
    query: String,
    done_for: String,
    last_edit: f64,
    hits: Vec<GsHit>,
    opened: bool,
}

struct GsHit {
    session: u64,
    line: i32,
    text: String,
}

struct PendingWt {
    rx: std::sync::mpsc::Receiver<Result<(), String>>,
    opts: NewOpts,
    wt: crate::state::Worktree,
}

struct WtClose {
    id: u64,
    changes: usize,
    /// Läuft noch die Prüfung auf offene Änderungen (`git status` im Hintergrund, #259)?
    changes_rx: Option<std::sync::mpsc::Receiver<usize>>,
    discard: bool,
    /// Hintergrundprozesse im Job der Sitzung beim Öffnen (#168, #213).
    lingering: Vec<closing::Lingering>,
    /// „Hintergrundprozesse beenden“ angehakt.
    end_jobs: bool,
}

impl WtClose {
    /// Ergebnis der Prüfung abholen; `true`, solange sie noch läuft.
    fn checking(&mut self) -> bool {
        let Some(rx) = &self.changes_rx else { return false };
        match rx.try_recv() {
            Ok(n) => self.changes = n,
            Err(std::sync::mpsc::TryRecvError::Empty) => return true,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {} // wie bisher bei einem Fehler: keine Änderungen bekannt
        }
        self.changes_rx = None;
        false
    }
}

struct Palette {
    query: String,
    sel: usize,
}

fn fuzzy(query: &str, text: &str) -> Option<i32> {
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let mut score = 0;
    let mut pos = 0usize;
    let mut last: Option<usize> = None;
    for qc in query.to_lowercase().chars().filter(|c| !c.is_whitespace()) {
        let found = (pos..t.len()).find(|&i| t[i] == qc)?;
        score += if last == Some(found.wrapping_sub(1)) { 3 } else { 1 };
        if found == 0 || t[found - 1] == ' ' {
            score += 2;
        }
        last = Some(found);
        pos = found + 1;
    }
    Some(score - (t.len() as i32 / 8))
}

pub struct App {
    settings: Settings,
    shells: Vec<ShellSpec>,
    themes: Vec<Theme>,
    /// Theme-Dateien, die nicht geladen werden konnten (Grund für die Einstellungen und `theme_list`).
    theme_issues: Vec<theme::Issue>,
    /// Theme-Editor und Verwaltung eigener Themes (#309).
    theme_ui: theme_editor::ThemeUi,
    /// Eigener Hintergrund des Themes je Fenster (#387).
    backdrops: backdrop::Backdrops,
    entries: BTreeMap<u64, Entry>,
    layout: Option<Node>,
    focused: u64,
    next_id: u64,
    /// Nächste Kennung eines Seitenleisten-Projekts (#386, nie wiederverwendet).
    next_project_id: u64,
    want_focus: Option<u64>,
    /// Zurück in den Terminal Hub (#238): Zahl der Bilder seit dem Umschalten; nach zwei vollen Terminal-Bildern bekommt
    /// das zuvor fokussierte Terminal (sonst die Pane oben links) den Fokus – nur, wenn kein Dialog/Popup offen ist.
    focus_return: Option<u8>,
    rename: Option<(u64, String)>,
    group_prompt: Option<Prompt>,
    templates: Vec<Template>,
    templates_open: bool,
    tpl_name: String,
    /// Vorlagen-Bibliothek der Seitenleiste (#244), je Fenster.
    tpl: tpl_lib::TplView,
    cost_open: bool,
    budget_warned_day: String,
    last_budget_check: Instant,
    history_hits: Option<Vec<(history::HistoryItem, String)>>,
    history_hits_rx: Option<std::sync::mpsc::Receiver<Vec<(history::HistoryItem, String)>>>,
    scale_pending: Option<f32>,
    folder_prompt: Option<FolderPrompt>,
    folder_rects: Vec<(String, Rect)>,
    tree_geo: TreeGeo,
    /// Hinweise aus `spawn_into` (z. B. unbrauchbarer gespeicherter WSL-Ordner), im nächsten Bild als Meldung gezeigt.
    spawn_notes: std::cell::RefCell<Vec<String>>,
    new_dialog: Option<NewDialog>,
    /// Dialog „Neuer Chat“ (Server und Modell wählen).
    new_chat: Option<models_ui::NewChatDlg>,
    confirm_close: Option<closing::CloseAsk>,
    /// Rückfrage „Sitzung neu laden?“ (#169).
    reload_ask: Option<session_settings::ReloadAsk>,
    /// Auswahl der Konversation vor Neu laden/Neustart/Programmstart (#302); global, erscheint im Hauptfenster.
    conv_pick: conv_pick::ConvPick,
    /// Kopierte Umgebungsvariablen einer Sitzung (#384, Kontextmenü): bleiben in Paneldeck, nie in der Zwischenablage.
    env_clip: Option<Vec<crate::session_env::EnvVar>>,
    /// Rückfrage *Neu starten / Jetzt setzen / Später* nach geänderten Variablen einer laufenden Sitzung (#384).
    env_ask: Option<(u64, env_ui::EnvLive)>,
    /// Rückfrage vor dem Einfügen mehrerer Zeilen (#183).
    paste_ask: Option<paste_ask::PasteAsk>,
    /// Dialog „Einstellungen der Sitzung“ (#170).
    session_settings: Option<session_settings::SessionForm>,
    settings_open: bool,
    /// „Sitzungen merken“ beim Start (an → beim Beenden mit „aus“ vorher sichern, siehe `fresh_start`).
    remember_at_start: bool,
    /// Rückfrage „Sitzungen merken ausschalten?“ offen – in diesem Fenster (Id, 0 = Hauptfenster).
    remember_off_ask: Option<u64>,
    dir: PathBuf,
    last_save: Instant,
    applied_theme: String,
    /// Fertige egui-Stile je Schlüssel von `applied_theme` (Fenster mit anderer Akzentfarbe tauschen nur den Zeiger).
    style_cache: Vec<(String, std::sync::Arc<egui::Style>)>,
    last_title: String,
    mode: titlebar::Mode,
    db: Option<crate::db::Db>,
    db_error: Option<String>,
    tasks: tasks_ui::TasksUi,
    plan: planning_ui::PlanUi,
    /// Kalender (#164, `calendar_ui`).
    cal: calendar_ui::CalUi,
    /// Ticket↔Terminal-Verknüpfung (`terminal_links.rs`).
    tlinks: terminal_links::TLinks,
    backup_rx: Option<std::sync::mpsc::Receiver<Result<crate::backup::BackupInfo, String>>>,
    backup_checked: Instant,
    backup_msg: Option<String>,
    backups_cache: Option<Vec<crate::backup::BackupInfo>>,
    /// Fenster „Aufräumen …“ bzw. Ergebnis eines beim Start ausgeführten Aufräumens (#307).
    cleanup: Option<cleanup_ui::CleanupUi>,
    backups_dirty: bool,
    confirm_restore: Option<PathBuf>,
    restore_error: Option<String>,
    hook_server: Option<HookServer>,
    hooks_file: Option<PathBuf>,
    hooks_global: bool,
    /// Die globalen Hooks zeigen auf eine andere paneldeck.exe (Pfad): deren Meldungen tragen keinen Schlüssel und zählen
    /// nur für den Status (#260); die Einstellungen bieten „Neu eintragen“ an.
    hooks_foreign: Option<String>,
    /// Die globalen Hooks stammen von einer älteren Version und decken nicht alle Ereignisse ab: Sitzungen bekommen die
    /// Hooks dann zusätzlich per `--settings`.
    hooks_incomplete: bool,
    /// Letzte Gegenprobe des Claude-Status (`claude_status_tick`).
    last_status_check: Instant,
    hooks_msg: String,
    exe: PathBuf,
    palette: Option<Palette>,
    prompts: Vec<Snippet>,
    prompts_open: bool,
    prompt_sel: usize,
    broadcast: bool,
    notes_open: Option<u64>,
    pane_rects: HashMap<u64, Rect>,
    usage: UsageWatcher,
    git: GitWatcher,
    history_open: bool,
    history: Vec<HistoryItem>,
    history_rx: Option<std::sync::mpsc::Receiver<Vec<HistoryItem>>>,
    history_filter: String,
    last_sync: Instant,
    /// Letzte Suche nach Transkript-Dateien (Verzeichnis ~/.claude/projects durchsuchen ist teuer).
    last_transcript_scan: Instant,
    files_open: bool,
    files_hidden: bool,
    tree_cache: HashMap<PathBuf, Vec<TreeItem>>,
    tree_stamp: Instant,
    tree_root: String,
    projects: Vec<Project>,
    active_project: u64,
    dragging: Option<u64>,
    /// Ordner, der gerade aus dem Baum gezogen wird.
    dragging_folder: Option<String>,
    filter: String,
    text_prompt: Option<TextPrompt>,
    tray: Option<Tray>,
    /// Agenten-Schnittstelle (MCP): wartende Aufrufe, Protokoll, Aktivität.
    mcp: mcp::McpState,
    tray_rx: std::sync::mpsc::Receiver<TrayCmd>,
    quitting: bool,
    /// Beenden trotz Hintergrundprozessen bestätigt (#168, `closing::App::quit_guard`): nicht noch einmal fragen.
    quit_ok: bool,
    /// Prozessliste (#243): Einträge, Laufzeit, Ausgabeansicht, Dialoge.
    procs: procs_ui::ProcsState,
    /// Aktionen beim Speichern im Editor (#383).
    save_actions: save_actions_ui::SaState,
    /// Dienste des Editor-Reiters (#54): Plattenänderungen, Suche in Dateien, `--edit`, Explorer-Eintrag.
    editor_svc: editor_services::EditorServices,
    detached: Vec<u64>,
    settings_tab: usize,
    capture_key: Option<String>,
    queue_open: Option<u64>,
    queue_draft: String,
    pending_actions: Vec<Action>,
    start_in_tray: bool,
    win_autostart: crate::integration::Status,
    win_context: crate::integration::Status,
    show_licenses: bool,
    win_probed: bool,
    composer_open: bool,
    composer_text: String,
    composer_keep: bool,
    composer_focus: bool,
    profile_sel: usize,
    diff: DiffWatcher,
    diff_open: bool,
    diff_sel: Option<(String, bool)>,
    commit_msg: String,
    /// Vom Commit-Thread gesetzt, wenn der Commit geklappt hat – erst dann wird die Nachricht geleert.
    commit_ok: std::sync::Arc<std::sync::atomic::AtomicBool>,
    bg_tx: std::sync::mpsc::Sender<String>,
    bg_rx: std::sync::mpsc::Receiver<String>,
    pending_wt: Option<PendingWt>,
    wt_close: Option<WtClose>,
    /// Jobs geschlossener/neu gestarteter Sitzungen mit übrigen Prozessen (#218, #213) und letzte Pflege.
    retired: Vec<closing::Retired>,
    retired_poll: Option<Instant>,
    gsearch: Option<GlobalSearch>,
    notice: Option<(String, Instant)>,
    /// Toast-Stapel unten rechts (`toasts.rs`), Uhr dafür und Toast unter dem Mauszeiger.
    toasts: toasts::ToastQueue,
    toasts_epoch: Instant,
    toasts_hover: Option<u64>,
    /// Zustand je Sitzung für OSC 52 (#181): wartender Text, Ratenbegrenzung, Toasts.
    osc52: HashMap<u64, osc52::ClipState>,
    /// Ratenbegrenzung der Programm-Benachrichtigungen und der Glocke je Sitzung (#183, `vt_notify.rs`).
    vt_limits: HashMap<u64, crate::vtnotify::RateLimit>,
    /// Zuletzt an die Taskleiste gemeldeter Fortschritt je Fenster (#183).
    taskbar: crate::platform::taskbar::Taskbar,
    /// Offene Rückfrage „Link öffnen?“ für OSC-8-Links mit anderem Schema als http/https (#183, `link_ask.rs`).
    link_ask: Option<link_ask::LinkAsk>,
    hwnd: Option<isize>,
    in_tray: bool,
    editor: Option<Editor>,
    /// Reiter „Editor“ (#54): Ordnerbaum, Dokumente, Teilungen. Für alle Fenster gemeinsam (nicht in `view_fields!`).
    ed: editor_ui::EditorUi,
    /// Reiter „Zeit“ (#164): Daten, Auswahl, Klingeln, Hinweise. Für alle Fenster gemeinsam (nicht in `view_fields!`).
    time: time_ui::TimeUi,
    applied_font: String,
    /// Weitere Fenster (`multiwin.rs`, #110).
    wins: multiwin::Wins,
    /// Selbst-Update aus den GitHub-Releases (`update/`).
    updater: update::Updater,
    /// Erzeugt die Antworten aller Chats (wählt Server und Modell anhand von `ChatMeta`).
    chat_backend: std::sync::Arc<dyn ChatBackend>,
    /// Modelllisten der LLM-Server im Hintergrund abrufen („Verbinden“).
    llm_conn: models_ui::Connector,
    /// Zustand des Reiters Einstellungen → Modelle. Bewusst für alle Fenster gemeinsam (nicht in `view_fields!`): Er
    /// bearbeitet die globalen Einstellungen, und `dirty_since` steuert das verzögerte Speichern im Hauptdurchgang
    /// (`models_save_tick`) – je Fenster getauscht, bliebe eine Änderung in einem gerade nicht gezeichneten Fenster
    /// ungespeichert. Folge: Ein offenes Server-Formular erscheint in jedem Fenster, das diesen Reiter zeigt.
    models_ui: models_ui::ModelsUi,
    /// Linux/macOS: Eingaben der Master-Passwort-Frage (`secret_ui`, im Speicher überschrieben).
    #[cfg(unix)]
    master_pw: secret_ui::PwForm,
    /// Interaktives Tutorial (#143): laufende Tour, Fortschritt (`tutorial.json`), Karte beim ersten Start.
    tut: tutorial::Tut,
}

fn home_dir() -> String {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default()
}

/// Hinweis: aus alten Claude-Argumenten der Sitzung `name` weggelassene Flags (Review zu #245).
fn legacy_dropped_note(name: &str, dropped: &[String]) -> String {
    format!(
        "{} ({name}): {}",
        state::t(
            "Alte Claude-Argumente werden nicht mehr unterstützt oder sind ungültig, weggelassen",
            "Old Claude arguments are no longer supported or invalid, left out"
        ),
        dropped.join(", ")
    )
}

/// Hinweis: die globalen Hooks gehören zu einer anderen paneldeck.exe (`other`, #260).
fn hooks_foreign_note(other: &str) -> String {
    format!(
        "{} ({}). {}",
        state::t(
            "Die globalen Claude-Hooks in ~/.claude/settings.json zeigen auf eine andere paneldeck.exe",
            "The global Claude hooks in ~/.claude/settings.json point to another paneldeck.exe"
        ),
        crate::mcp::short(other, 80),
        state::t(
            "Ihre Meldungen zählen nur für den Status. Einstellungen → Claude → Hooks: „Neu eintragen“.",
            "Their messages only count for the status. Settings → Claude → Hooks: “Reinstall”."
        ),
    )
}

fn setup_fonts(ctx: &egui::Context, choice: &str, editor_font: &str) {
    let mut fonts = build_fonts(choice);
    // Schrift des Editor-Reiters (#54): eigene Familie „editor“, Standard = Terminalschrift.
    editor_services::add_editor_family(&mut fonts, editor_font, choice);
    ctx.set_fonts(fonts);
}

/// Baut die Schriftdefinitionen der App (eingebettete + vorhandene Systemschriften); getrennt von `setup_fonts`,
/// damit Tests die Glyph-Abdeckung ohne Fenster prüfen können.
fn build_fonts(choice: &str) -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    // Cascadia Mono (SIL OFL) ist eingebettet und die bevorzugte Terminal-Schrift; Hack bleibt Fallback.
    fonts.font_data.insert(
        "cascadia".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!("../../assets/fonts/CascadiaMono-Regular.ttf"))),
    );
    fonts.families.entry(egui::FontFamily::Monospace).or_default().insert(0, "cascadia".to_owned());
    // Lucide-Icons (ISC) als eigene Familie nur für Icons (Icon-Explorer, `lucide:<name>`).
    planning_ui::install_icon_font(&mut fonts);
    // Optionale Systemschriften als Fallback für Symbole, Sonderzeichen, CJK, Thai, Indisch, Mathe usw.
    // Es werden nur vorhandene Dateien geladen; fehlt eine, ändert sich nichts.
    let sys = crate::platform::fonts::system_font;
    let candidates: [(&str, Option<std::path::PathBuf>); 20] = [
        ("seguisym", sys("seguisym.ttf")),
        ("segoeui", sys("segoeui.ttf")),
        ("msyh", sys("msyh.ttc")),
        ("msjh", sys("msjh.ttc")),
        ("yugoth", sys("YuGothM.ttc")),
        ("meiryo", sys("meiryo.ttc")),
        ("malgun", sys("malgun.ttf")),
        ("simsun", sys("simsun.ttc")),
        ("nirmala", sys("Nirmala.ttc")),
        ("leelawui", sys("LeelawUI.ttf")),
        ("seguihis", sys("seguihis.ttf")),
        ("cambria", sys("cambria.ttc")),
        ("arial", sys("arial.ttf")),
        ("dejavu", Some("/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf".into())),
        // macOS (#150): Symbole, breite Unicode-Abdeckung, CJK, Koreanisch, Thai, Mathe
        ("applesymbols", sys("Apple Symbols.ttf")),
        ("arialuni", sys("Arial Unicode.ttf")),
        ("hiragino", sys("Hiragino Sans GB.ttc")),
        ("applesdgothic", sys("AppleSDGothicNeo.ttc")),
        ("thonburi", sys("Thonburi.ttc")),
        ("stixmath", sys("STIXTwoMath.otf")),
    ];
    for (name, path) in candidates {
        if let Some(Ok(bytes)) = path.map(std::fs::read) {
            fonts.font_data.insert(name.to_owned(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
            for fam in [egui::FontFamily::Monospace, egui::FontFamily::Proportional] {
                fonts.families.entry(fam).or_default().push(name.to_owned());
            }
        }
    }
    // UI-Schrift: Segoe UI (Windows-Systemschrift) vor allen anderen, sonst bleibt egui-Standard.
    if fonts.font_data.contains_key("segoeui") {
        let fam = fonts.families.entry(egui::FontFamily::Proportional).or_default();
        fam.retain(|n| n != "segoeui");
        fam.insert(0, "segoeui".to_owned());
    }
    // Fette UI-Schrift (Titel der Todo-Karten): Segoe UI Bold, sonst die normale UI-Schrift als Ersatz.
    let mut bold: Vec<String> = Vec::new();
    if let Some(Ok(bytes)) = sys("segoeuib.ttf").map(std::fs::read) {
        fonts.font_data.insert("segoeuib".to_owned(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
        bold.push("segoeuib".to_owned());
    }
    bold.extend(fonts.families.get(&egui::FontFamily::Proportional).cloned().unwrap_or_default());
    fonts.families.insert(egui::FontFamily::Name("bold".into()), bold);
    // Gewählte Terminalschrift an die erste Stelle setzen (Fallbacks bleiben dahinter): eingebettete Cascadia Mono,
    // Hack (egui) oder jede installierte Festbreitenschrift, z. B. Nerd Fonts (#183, `sysfonts`).
    let key = match choice {
        "" | "Cascadia Mono" => None,
        "Hack" => Some("Hack".to_string()),
        other => match crate::sysfonts::load(other, false, false).filter(crate::sysfonts::is_monospace) {
            Some(d) => {
                fonts.font_data.insert("term-regular".to_owned(), std::sync::Arc::new(egui::FontData { font: d.bytes.into(), index: d.index, tweak: Default::default() }));
                Some("term-regular".to_string())
            }
            // Ohne GDI (oder nicht gefunden): die früheren festen Dateien.
            None => {
                let file = match other {
                    "Consolas" => Some(("consolas", "consola.ttf")),
                    "Lucida Console" => Some(("lucon", "lucon.ttf")),
                    "Courier New" => Some(("cour", "cour.ttf")),
                    _ => None,
                };
                file.and_then(|(k, f)| {
                    sys(f).and_then(|p| std::fs::read(p).ok()).map(|b| {
                        fonts.font_data.insert(k.to_owned(), std::sync::Arc::new(egui::FontData::from_owned(b)));
                        k.to_string()
                    })
                })
            }
        },
    };
    if let Some(k) = key {
        let fam = fonts.families.entry(egui::FontFamily::Monospace).or_default();
        fam.retain(|n| n != &k);
        fam.insert(0, k);
    }
    // Schnitte fett/kursiv/fett-kursiv der Terminalschrift (SGR 1/3). Familien gibt es immer; ohne echten Schnitt stehen
    // dort nur die normalen Schriften, das Terminal zeichnet dann nach (`sysfonts::plan`).
    let mono = fonts.families.get(&egui::FontFamily::Monospace).cloned().unwrap_or_default();
    let lookup = if choice.is_empty() { "Cascadia Mono" } else { choice };
    let mut real = [false; 3];
    for (i, (fam_name, bold, italic)) in [(crate::sysfonts::FAMILY_BOLD, true, false), (crate::sysfonts::FAMILY_ITALIC, false, true), (crate::sysfonts::FAMILY_BOLD_ITALIC, true, true)].into_iter().enumerate() {
        let mut list = mono.clone();
        if let Some(d) = crate::sysfonts::load(lookup, bold, italic).filter(|d| crate::sysfonts::face_matches(d, bold, italic)) {
            fonts.font_data.insert(fam_name.to_owned(), std::sync::Arc::new(egui::FontData { font: d.bytes.into(), index: d.index, tweak: Default::default() }));
            list.insert(0, fam_name.to_owned());
            real[i] = true;
        }
        fonts.families.insert(egui::FontFamily::Name(fam_name.into()), list);
    }
    crate::sysfonts::set_real_variants(real[0], real[1], real[2]);
    // Familie „editor“ gibt es immer (Standard: wie die Terminalschrift), damit sie nie fehlt (#54).
    editor_services::add_editor_family(&mut fonts, "", choice);
    fonts
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Schrift wird in update() über apply_font gesetzt.
        let dir = state::data_dir();
        // Vorgemerktes Aufräumen (#307) zuerst: es kann Einstellungen, Zustand und Datenbank betreffen.
        let cleanup_report = crate::cleanup::apply_pending(&dir, || {
            let s: Settings = state::load_json(&dir.join("settings.json")).unwrap_or_default();
            crate::cli::get().backup_dir.clone().unwrap_or_else(|| crate::backup::dir(&dir, &s.backup_dir))
        });
        let mut settings: Settings = state::load_json(&dir.join("settings.json")).unwrap_or_default();
        settings.sanitize();
        // `--lang`: nur für diesen Start (beim Speichern bleibt die gespeicherte Sprache, siehe `state::ser_language`).
        if let Some(l) = crate::cli::get().lang {
            state::set_lang_override(l, settings.language);
            settings.language = l;
        }
        state::set_lang(settings.language);
        // Vor dem Wiederherstellen der Sitzungen: welche ConPTY neue Sitzungen bekommen (#183).
        crate::platform::conpty::USE_BUNDLED.store(settings.bundled_conpty, std::sync::atomic::Ordering::Relaxed);
        let (mut saved, startup_note) = state::load_saved(&dir.join("state.json"));
        // „Sitzungen merken“ aus (#157): frischer Start. Steht in state.json noch etwas (Absturz nach dem Ausschalten, von
        // Hand geändert), wird die Datei vorher gesichert. Scrollback und Chatverläufe früherer Läufe werden gelöscht – auch
        // nach einem Absturz, bei dem `save_for_exit` nicht mehr lief. Ausnahme: Neustart nach einem Update – dann werden
        // die Sitzungen einmal wiederhergestellt (`update::install::restore_once`, die Variable ist schon entfernt; beim
        // nächsten Beenden bzw. nach einem Absturz beim nächsten Start gilt wieder der frische Start).
        let remember_at_start = settings.remember_sessions;
        let restored_once = fresh_start::restore_once_applies(settings.remember_sessions, update::install::restore_once());
        if !settings.remember_sessions && !restored_once {
            saved = fresh_start::start_fresh(&dir, saved, state::persisted_lang(settings.language));
        }
        let restored_once = restored_once && !saved.sessions.is_empty();
        // Vorgemerkte Wiederherstellung einspielen, dann die Datenbank öffnen (Migrationen inklusive).
        let restore_result = crate::backup::apply_pending_restore(&dir, crate::db::FILE);
        // Sicherung von vor 3.2.0 (Schema < 13, #386): Zugriff der Ticket-Projekte steht danach auf den Standards.
        let restore_resets_access = matches!(restore_result, Some(Ok(_))) && mcp::access_gate::restore_resets_access(mcp::access_gate::file_schema_version(&dir.join(crate::db::FILE)));
        let backup_dir = crate::cli::get().backup_dir.clone().unwrap_or_else(|| crate::backup::dir(&dir, &settings.backup_dir));
        let (db, db_error) = match crate::db::open(&dir, &backup_dir) {
            Ok(d) => (Some(d), None),
            Err(e) => (None, Some(e.message(settings.language))),
        };
        let shells = shells::detect();
        crate::shellinit::init(&dir);
        // Login-Shells laufender WSL-Distributionen im Hintergrund ermitteln (#57; andere beim ersten Sitzungsstart).
        if shells.iter().any(shells::is_wsl) {
            crate::shellinit::detect_running_wsl();
        }
        let bg = std::sync::mpsc::channel::<String>();
        let procs_state = procs_ui::ProcsState::load(&dir);
        let save_actions_state = save_actions_ui::SaState::load(&dir);
        let mut app = App {
            settings,
            shells,
            themes: Vec::new(),
            theme_issues: Vec::new(),
            theme_ui: theme_editor::ThemeUi::default(),
            backdrops: Default::default(),
            entries: BTreeMap::new(),
            layout: None,
            focused: 0,
            next_id: saved.next_id.max(1),
            next_project_id: saved.project_id_floor(),
            want_focus: None,
            focus_return: None,
            rename: None,
            group_prompt: None,
            templates: templates::load(&dir),
            templates_open: false,
            tpl_name: String::new(),
            tpl: tpl_lib::TplView::default(),
            cost_open: false,
            budget_warned_day: String::new(),
            last_budget_check: Instant::now(),
            history_hits: None,
            history_hits_rx: None,
            scale_pending: None,
            folder_prompt: None,
            folder_rects: Vec::new(),
            tree_geo: TreeGeo::default(),
            spawn_notes: Default::default(),
            new_dialog: None,
            new_chat: None,
            confirm_close: None,
            reload_ask: None,
            conv_pick: Default::default(),
            env_clip: None,
            env_ask: None,
            paste_ask: None,
            session_settings: None,
            settings_open: false,
            remember_at_start,
            remember_off_ask: None,
            dir,
            last_save: Instant::now(),
            applied_theme: String::new(),
            style_cache: Vec::new(),
            last_title: String::new(),
            mode: titlebar::Mode::default(),
            db,
            db_error,
            tasks: tasks_ui::TasksUi::default(),
            plan: planning_ui::PlanUi::default(),
            cal: calendar_ui::CalUi::default(),
            backup_rx: None,
            backup_checked: Instant::now().checked_sub(Duration::from_secs(61)).unwrap_or_else(Instant::now),
            backup_msg: None,
            backups_cache: None,
            cleanup: cleanup_report.map(cleanup_ui::CleanupUi::with_report),
            backups_dirty: true,
            confirm_restore: None,
            restore_error: None,
            hook_server: HookServer::start(&cc.egui_ctx),
            hooks_file: None,
            hooks_global: false,
            hooks_foreign: None,
            hooks_incomplete: false,
            last_status_check: Instant::now(),
            hooks_msg: String::new(),
            // Linux-AppImage: die `.AppImage`-Datei (Selbst-Update, Neustart, Integrationen), nicht das eingehängte Abbild.
            #[cfg(all(unix, not(target_os = "macos")))]
            exe: crate::platform::desktop::this_exe(),
            #[cfg(not(all(unix, not(target_os = "macos"))))]
            exe: std::env::current_exe().unwrap_or_default(),
            palette: None,
            tlinks: Default::default(),
            prompts: Vec::new(),
            prompts_open: false,
            prompt_sel: 0,
            broadcast: false,
            notes_open: None,
            pane_rects: HashMap::new(),
            usage: UsageWatcher::start(cc.egui_ctx.clone()),
            git: GitWatcher::start(cc.egui_ctx.clone()),
            history_open: false,
            history: Vec::new(),
            history_rx: None,
            history_filter: String::new(),
            last_sync: Instant::now() - Duration::from_secs(10),
            last_transcript_scan: Instant::now() - Duration::from_secs(60),
            files_open: false,
            files_hidden: false,
            tree_cache: HashMap::new(),
            tree_stamp: Instant::now(),
            tree_root: String::new(),
            projects: saved.projects.clone(),
            active_project: saved.active_project,
            dragging: None,
            dragging_folder: None,
            filter: String::new(),
            text_prompt: None,
            tray: None,
            mcp: mcp::McpState::default(),
            tray_rx: std::sync::mpsc::channel().1,
            quitting: false,
            quit_ok: false,
            procs: procs_state,
            save_actions: save_actions_state,
            editor_svc: editor_services::EditorServices::from_cli(),
            detached: saved.detached.clone(),
            settings_tab: 0,
            capture_key: None,
            queue_open: None,
            queue_draft: String::new(),
            pending_actions: Vec::new(),
            start_in_tray: crate::cli::get().tray,
            win_autostart: crate::integration::Status::Off,
            win_context: crate::integration::Status::Off,
            show_licenses: false,
            win_probed: false,
            composer_open: false,
            composer_text: saved.composer_draft.clone(),
            composer_keep: false,
            composer_focus: false,
            profile_sel: 0,
            diff: DiffWatcher::start(cc.egui_ctx.clone()),
            diff_open: false,
            diff_sel: None,
            commit_msg: String::new(),
            commit_ok: Default::default(),
            bg_tx: bg.0,
            bg_rx: bg.1,
            pending_wt: None,
            wt_close: None,
            retired: Vec::new(),
            retired_poll: None,
            gsearch: None,
            notice: None,
            toasts: Default::default(),
            toasts_epoch: Instant::now(),
            toasts_hover: None,
            osc52: HashMap::new(),
            vt_limits: HashMap::new(),
            taskbar: Default::default(),
            link_ask: None,
            hwnd: crate::platform::window::hwnd_from(cc),
            in_tray: false,
            editor: None,
            ed: Default::default(),
            time: Default::default(),
            applied_font: String::new(),
            wins: Default::default(),
            // `--no-update-check`: keine automatische Prüfung für diesen Start (die manuelle geht weiter).
            updater: update::Updater::new(!crate::cli::get().no_update_check),
            chat_backend: std::sync::Arc::new(chatmod::LlmBackend::new()),
            llm_conn: Default::default(),
            models_ui: Default::default(),
            #[cfg(unix)]
            master_pw: Default::default(),
            tut: Default::default(),
        };
        // `--no-tray`: kein Tray-Symbol; Schließen/Minimieren verhalten sich dann normal (siehe `handle_tray`).
        if !crate::cli::get().no_tray {
            let (tx, rx) = std::sync::mpsc::channel();
            let l = app.settings.language;
            app.tray = Tray::create(&cc.egui_ctx, tx, l.t("Paneldeck anzeigen", "Show Paneldeck"), l.t("Beenden", "Quit"));
            app.tray_rx = rx;
        }
        if let Some(h) = app.hwnd {
            crate::platform::chrome::init(h, &cc.egui_ctx); // runde Ecken + Snap-Layouts am Maximieren-Knopf
        }
        clip::init(&app.dir);
        crate::afford::install(&cc.egui_ctx); // Hand-Zeiger über allem, was wie ein Knopf funktioniert (#187)
        app.reload_themes();
        app.prompts = state::load_json(&app.dir.join("prompts.json")).unwrap_or_else(|| prompts::defaults(state::persisted_lang(app.settings.language)));
        if let Some(h) = &app.hook_server {
            hooks::write_ipc_file(&app.dir, h.port, &h.ipc_token);
        }
        app.hooks_file = hooks::write_session_settings(&app.dir, &app.exe);
        app.refresh_hooks_global();
        if let Some(other) = &app.hooks_foreign {
            app.spawn_notes.borrow_mut().push(hooks_foreign_note(other));
        }
        if app.settings.default_shell.is_empty() {
            app.settings.default_shell = app.shells.first().map(|s| s.id.clone()).unwrap_or_default();
        }

        // Sitzungen wiederherstellen
        for meta in saved.sessions {
            app.next_id = app.next_id.max(meta.id.saturating_add(1));
            let id = meta.id;
            if meta.kind == SessionKind::Chat {
                // Chat: Verlauf aus chats/<id>.json, keine PTY.
                let (file, warning) = chatmod::store::load(&chatmod::store::path(&app.dir, id));
                // Keine Id neu vergeben, auf die eine dauerhafte Freigabe zeigt (sonst gälte sie für ein neues Terminal).
                app.next_id = app.next_id.max(chatmod::store::max_rule_session(&file).saturating_add(1));
                let chat = ChatState::new(file, warning);
                app.entries.insert(id, Entry::new_chat(meta, chat));
                continue;
            }
            let mut entry = Entry::new(meta);
            // „Konversation auswählen“ (#302): erst die Auswahl (eine zur Zeit), dann der Start.
            if !app.begin_pick(&cc.egui_ctx, &mut entry, true, false) {
                app.spawn_into(&cc.egui_ctx, &mut entry, Launch { restore: true, restart: false, continue_last: false, prompt: None, resume_id: None });
            }
            app.entries.insert(id, entry);
        }
        // Agenten-Ketten: Sitzungen, die es nicht mehr gibt, fallen heraus (#152).
        taint::prune(&mut app.entries);
        // Keine Id vergeben, zu der noch Verlauf/Scrollback daliegt (Absturz vor dem Speichern, zurückgesetzte state.json).
        app.next_id = app.next_id.max(chatmod::store::max_used_id(&app.dir).saturating_add(1));
        // Projekte: ältere Stände ohne Projekte bekommen ein Standardprojekt.
        if app.projects.is_empty() {
            app.projects.push(Project {
                id: 1,
                // gespeichert (state.json): in der gespeicherten Sprache, nicht in der von `--lang`
                name: state::persisted_lang(app.settings.language).t("Standard", "Default").to_string(),
                layout: saved.layout.clone(),
                focused: saved.focused,
                ..Project::default()
            });
            app.active_project = 1;
        }
        if !app.projects.iter().any(|p| p.id == app.active_project) {
            app.active_project = app.projects[0].id;
        }
        let first_project = app.projects[0].id;
        let valid: Vec<u64> = app.projects.iter().map(|p| p.id).collect();
        for e in app.entries.values_mut() {
            if !valid.contains(&e.meta.project) {
                e.meta.project = first_project;
            }
        }
        let ppp = cc.egui_ctx.native_pixels_per_point().unwrap_or(1.0) * app.settings.ui_scale;
        app.restore_windows(&saved.extra_windows, ppp);
        let p = app.projects.iter().find(|p| p.id == app.active_project).cloned().unwrap_or_default();
        app.load_project_view(p.layout, p.focused);
        app.detached.retain(|id| app.entries.contains_key(id));
        let det = app.detached.clone();
        for id in det {
            if app.layout.as_ref().is_some_and(|l| l.contains(id)) {
                app.hide_pane(id);
            }
        }
        // Ohne wiederhergestellte Sitzungen startet Paneldeck ohne Terminal (leerer Hub mit Startknöpfen, #111).
        let tr: Vec<(u64, String)> = app.entries.iter().map(|(i, e)| (*i, e.meta.transcript.clone())).collect();
        for (i, t) in tr {
            if !t.is_empty() {
                app.usage.set_path(i, PathBuf::from(t));
            }
        }
        // Start aus dem Explorer-Kontextmenü: Sitzung im übergebenen Ordner öffnen.
        let cli = crate::cli::get();
        if let Some(p) = &cli.open {
            let _ = app.open_folder(&cc.egui_ctx, p, cli.claude, None);
        }
        app.want_focus = Some(app.focused);
        if let Some(n) = startup_note {
            app.notify_text(n);
        }
        let lang = app.settings.language;
        if restored_once {
            app.notify_text(lang.t(
                "Nach dem Update wiederhergestellt. „Sitzungen merken“ ist aus: Beim nächsten Beenden werden die Sitzungen verworfen.",
                "Restored after the update. “Remember sessions” is off: the sessions are discarded on the next exit.",
            ).to_string());
        }
        if let Some(b) = state::SETTINGS_RESET.get() {
            app.notify_text(format!("{} {}", lang.t("Einstellungen zurückgesetzt (--reset-settings). Sicherung:", "Settings reset (--reset-settings). Backup:"), b.display()));
        }
        if let Some(r) = restore_result {
            app.notify_text(match r {
                Ok(p) => format!("{} {}", lang.t("Sicherung wiederhergestellt:", "Backup restored:"), p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()),
                Err(e) => format!("{}: {e}", lang.t("Wiederherstellung fehlgeschlagen", "Restore failed")),
            });
        }
        if restore_resets_access {
            app.notify_text(lang.t(
                "Die Sicherung stammt von vor 3.2.0: Der Zugriff der Agenten auf Ticket-Projekte steht auf den Standards (wie global, für alle) – prüfe Einstellungen → Agenten.",
                "The backup is from before 3.2.0: agent access to ticket projects was reset to the defaults (as global, for all) – check Settings → Agents.",
            ).to_string());
        }
        if let Some(e) = app.db_error.clone() {
            app.notify_text(e);
        }
        // Editor (#54): Ordner, Reiter, Teilungen und ungespeicherte Puffer (Hot Exit) des letzten Laufs.
        app.ed = editor_ui::EditorUi::restore(&app.dir);
        app
    }

    /// Neue Sitzung in einem Ordner (Dateien → deren Ordner).
    /// Ordner (oder Ordner einer Datei) als neue Sitzung öffnen; gibt deren Kennung zurück.
    /// `from_hub` = angestoßen aus einem Hub-Terminal (`paneldeck --open` dort, möglicherweise von einem Agenten): dann
    /// Optionen wie bei Sitzungen über MCP (#182): ohne die Rechte erweiternden Wertoptionen der Standards und mit
    /// ausdrücklichem Permission-Mode (`ClaudeOpts::for_agent`).
    fn open_folder(&mut self, ctx: &egui::Context, path: &str, claude: bool, origin: Option<u64>) -> u64 {
        let from_hub = origin.is_some();
        // Aus einer Sitzung heraus (#386): Fenster und Projekt dieser Sitzung (wie Kinder über `create_session`), nicht das
        // gerade angezeigte Projekt – sonst landete ein Agent über `paneldeck --open` in einem für ihn gesperrten Projekt.
        let (window, project) = mcp::access_gate::open_home(origin.and_then(|o| self.entries.get(&o)).map(|e| (e.meta.window, e.meta.project)), self.active_project);
        let p = std::path::Path::new(path);
        // Ein WSL-Pfad gilt ungeprüft als Ordner (schon das Prüfen kann die WSL-VM starten und die Oberfläche blockieren).
        let dir = if !crate::wsl::is_wsl_unc(path) && p.is_file() { p.parent().map(|d| d.to_string_lossy().to_string()).unwrap_or_default() } else { path.to_string() };
        // Ordner in einer WSL-Distribution (`\\wsl.localhost\<Distro>\…`, `\\wsl$\…`, z. B. „Hier öffnen“ im Explorer):
        // WSL-Sitzung dieser Distribution im passenden Linux-Pfad (#172). Unbekannte Distribution: wie bisher.
        // Das Standardprofil des Projekts kann ein eigenes Programm haben: nur wenn die Sitzung wirklich in dieser
        // Distribution startet, den Linux-Pfad nehmen – sonst bleibt es der (für Windows gültige) UNC-Pfad.
        let wsl = crate::wsl::unc_to_linux(&dir).and_then(|(d, linux)| self.wsl_shell_for(&d).map(|sid| (d, sid, linux))).filter(|(d, sid, _)| {
            let eff = self.effective_shell_id(Some(sid), project);
            matches!(folder_picker::Target::for_shell(self.shell_by_id(&eff).as_ref(), false), folder_picker::Target::Wsl { distro: Some(x), .. } if x.eq_ignore_ascii_case(d))
        }).map(|(_, sid, linux)| (sid, linux));
        let (shell_id, cwd) = match wsl {
            Some((sid, linux)) => (Some(sid), linux),
            None => (None, dir),
        };
        let claude_opts = from_hub.then(|| ClaudeOpts::default().for_agent(&self.settings.claude_defaults));
        let home = from_hub.then_some((Some(window), Some(project)));
        self.new_session(ctx, NewOpts { shell_id, cwd: Some(cwd), claude: claude && self.settings.claude_mode, claude_opts, window: home.and_then(|h| h.0), project: home.and_then(|h| h.1), ..NewOpts::default() })
    }

    fn handle_ipc(&mut self, ctx: &egui::Context) {
        let cmds: Vec<IpcCmd> = self.hook_server.as_ref().map(|h| h.cmd_rx.try_iter().collect()).unwrap_or_default();
        for c in cmds {
            self.from_tray(ctx);
            if let IpcCmd::Edit { path, origin } = c {
                // `paneldeck --edit <datei>` (#54): im Editor-Reiter öffnen (keine Sitzung, keine Agenten-Kette). Ein
                // Linux-Pfad gilt in der Sitzung, aus der der Aufruf kam (sonst in der WSL-Standard-Distribution, #392).
                let (from, _) = self.open_origin(origin.as_ref());
                match self.cli_path(&path, from) {
                    Ok(p) => self.editor_open_from_ipc(p),
                    Err(e) => self.notify_text(format!("{} {path}: {}", state::t("Datei ließ sich nicht im Editor öffnen:", "Could not open the file in the editor:"), e.message(self.settings.language))),
                }
                continue;
            }
            if let IpcCmd::Open { path, claude, origin } = c {
                // `paneldeck --open` aus einem Hub-Terminal heraus: Hat das Terminal eine Agenten-Kette, erbt die neue
                // Sitzung sie samt ihm (#152) – sonst entstünde so eine unmarkierte Claude-Sitzung. Ohne Kette (der
                // Nutzer tippt es) bleibt sie frei (`taint::on_create`). Absender nur über Job Object bzw. Prozessbaum
                // (#168); ein losgelöster, nicht zuzuordnender Absender darf keine Claude-Sitzung öffnen.
                let (origin, orphan) = self.open_origin(origin.as_ref());
                if orphan && claude {
                    self.notify_text(taint::orphan_error().1);
                    continue;
                }
                // Linux-Pfad (#392): in der Sitzung des Aufrufs (sonst WSL-Standard) – als `\\wsl.localhost\<D>\…` bzw.
                // `C:\…`; `open_folder` wählt dann die passende Shell.
                let path = self.cli_path(&path, origin).unwrap_or(path);
                let id = self.open_folder(ctx, &path, claude, origin);
                if origin.is_some() && taint::on_create(&mut self.entries, origin, id) {
                    self.save_state();
                }
            }
        }
        if self.start_in_tray {
            self.start_in_tray = false;
            if self.tray.is_some() {
                self.to_tray(ctx);
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            }
        }
    }

    /// Zusammenfassung für Fehlerberichte (wird per Knopf in die Zwischenablage kopiert).
    fn diagnostics(&self) -> String {
        let font_files = ["seguisym.ttf", "segoeui.ttf", "msyh.ttc", "msjh.ttc", "YuGothM.ttc", "meiryo.ttc", "malgun.ttf", "simsun.ttc", "Nirmala.ttc", "LeelawUI.ttf", "seguihis.ttf", "cambria.ttc", "arial.ttf", "consola.ttf"];
        let fonts: Vec<&str> = font_files.iter().copied().filter(|f| crate::platform::fonts::system_font(f).is_some_and(|p| p.exists())).collect();
        let crash = self.dir.join("crash.log");
        let l = self.settings.language;
        let t = |de: &'static str, en: &'static str| l.t(de, en);
        let lines = [
            format!("Paneldeck {}", env!("CARGO_PKG_VERSION")),
            format!("OS: {} {}", std::env::consts::OS, std::env::consts::ARCH),
            format!("EXE: {}", self.exe.display()),
            format!("{}: {}", t("Daten", "Data"), self.dir.display()),
            {
                let d = crate::paths::get();
                format!(
                    "{}: {} · {}: {} · {}: {}",
                    t("Datenordner-Quelle", "Data folder source"),
                    crate::paths::source_label(d.source, l),
                    t("Profil", "Profile"),
                    d.profile.as_deref().unwrap_or("–"),
                    t("Startoptionen", "Start overrides"),
                    {
                        let o = crate::cli::get().overrides();
                        if o.is_empty() { t("keine", "none").to_string() } else { o.join(" ") }
                    }
                )
            },
            format!("Shells: {}", self.shells.iter().map(|s| s.id.as_str()).collect::<Vec<_>>().join(", ")),
            format!(
                "{}: {} ({}: {}) · {}: {} · {}: {}",
                t("Sitzungen", "Sessions"),
                self.entries.len(),
                t("davon beendet", "of which exited"),
                self.entries.values().filter(|e| e.exited()).count(),
                t("Projekte", "Projects"),
                self.projects.len(),
                t("abgedockt", "detached"),
                self.detached.len()
            ),
            format!(
                "{}: {} · {}: {} · {}: {}",
                t("Hook-Server", "Hook server"),
                self.hook_server.as_ref().map(|h| format!("Port {}", h.port)).unwrap_or_else(|| t("aus", "off").into()),
                t("Hooks global", "Global hooks"),
                self.hooks_global,
                t("Claude-Modus", "Claude mode"),
                self.settings.claude_mode
            ),
            format!(
                "Theme: {} · {}: {} · {}: {:.0} % · {}: {} · {}: {:?}",
                self.settings.theme,
                t("Schrift", "Font"),
                self.settings.terminal_font,
                t("Deckkraft", "Opacity"),
                self.settings.opacity * 100.0,
                t("Animationen", "Animations"),
                self.settings.animations,
                t("Sprache", "Language"),
                self.settings.language
            ),
            format!("Tray: {} · {}: {}", self.tray.is_some(), t("Schließen minimiert", "Close minimizes"), self.settings.close_to_tray),
            format!("{}: {}", t("Systemschriften gefunden", "System fonts found"), if fonts.is_empty() { t("keine", "none").to_string() } else { fonts.join(", ") }),
            format!("{}: {}", t("abweichende Kürzel", "Custom shortcuts"), self.settings.keys.len()),
            format!(
                "{}: {}",
                t("Absturzprotokoll", "Crash log"),
                if crash.exists() { format!("{} ({})", t("vorhanden", "present"), crash.display()) } else { t("keins", "none").to_string() }
            ),
        ];
        lines.join("\n") + "\n"
    }

    /// Diagnose plus die letzten Zeilen des Absturzprotokolls – ein Klick für Fehlerberichte.
    fn error_report(&self) -> String {
        let mut s = self.diagnostics();
        if let Ok(log) = std::fs::read(self.dir.join("crash.log")) {
            let tail = &log[log.len().saturating_sub(6000)..];
            s.push_str("\n--- crash.log (Ende) ---\n");
            s.push_str(&String::from_utf8_lossy(tail));
        }
        s
    }

    fn themes_dir(&self) -> PathBuf {
        self.dir.join("themes")
    }

    /// Themes neu laden (eingebaute + `themes/*.json`). Ungültige Dateien landen mit Grund in `theme_issues` und sind nicht
    /// wählbar; ist das gewählte Theme darunter, gilt das Standard-Theme (#309). Leert den Stil-Zwischenspeicher, damit
    /// ein geändertes Theme gleichen Namens sofort wirkt.
    fn reload_themes(&mut self) {
        theme::write_example(&self.themes_dir());
        let mut t = theme::all();
        let (custom, issues) = theme::load_dir(&self.themes_dir(), &t);
        t.extend(custom);
        self.themes = t;
        self.theme_issues = issues;
        self.style_cache.clear();
        self.applied_theme.clear();
        self.backdrops.themes_changed();
    }

    fn binding(&self, id: &str) -> Option<(Modifiers, Key)> {
        match self.settings.keys.get(id).and_then(|s| keys::parse(s)) {
            // Eine früher aufgenommene Taste ohne Strg/Alt (z. B. „A“) wirkt nicht – sie finge jede Eingabe ab (#256).
            Some((m, k)) => keys::usable(m, k).then_some((m, k)),
            None => keys::DEFAULTS.iter().find(|(i, _)| *i == id).and_then(|(_, d)| keys::parse(d)),
        }
    }

    /// Aktuelles Kürzel als Text wie in `keys::DEFAULTS` (`Ctrl+Shift+D`; leer = keins) – für die Menüs des Editors.
    fn binding_raw(&self, id: &str) -> String {
        self.binding(id).map(|(m, k)| keys::format(m, k)).unwrap_or_default()
    }

    /// Anzeigetext des aktuellen Kürzels (für Tooltips).
    fn binding_text(&self, id: &str) -> String {
        let s = self.settings.keys.get(id).cloned().or_else(|| keys::DEFAULTS.iter().find(|(i, _)| *i == id).map(|(_, d)| d.to_string())).unwrap_or_default();
        keys::pretty(&s, self.settings.language == Lang::De)
    }

    /// Layout/Fokus des aktiven Projekts übernehmen (nur gültige Sitzungen des Projekts).
    fn load_project_view(&mut self, layout: Option<Node>, focused: u64) {
        let pid = self.active_project;
        // Unbekannte, fremde, abgedockte oder doppelte Blätter fallen weg; der Rest des Layouts bleibt (früher wurde ein
        // Layout mit nur einem ungültigen Blatt ganz verworfen).
        let layout = layout.and_then(|l| {
            let (entries, detached, win) = (&self.entries, &self.detached, self.wins.cur);
            l.normalize(|id| entries.get(&id).is_some_and(|e| e.meta.project == pid && e.meta.window == win) && !detached.contains(&id))
        });
        self.layout = layout;
        if self.layout.is_none() {
            // Kein Layout: die erste nicht minimierte Sitzung zeigen; sind alle minimiert, bleibt der leere Hub.
            if let Some(first) = self.unminimized_docked().first().copied() {
                self.layout = Some(Node::Leaf(first));
            }
        }
        self.focused = match &self.layout {
            Some(l) if l.contains(focused) => focused,
            Some(l) => {
                let mut v = Vec::new();
                l.leaves(&mut v);
                v.first().copied().unwrap_or(0)
            }
            None => 0,
        };
        self.want_focus = Some(self.focused);
    }

    fn project_ids(&self) -> Vec<u64> {
        self.entries.iter().filter(|(_, e)| e.meta.project == self.active_project && self.in_win(e)).map(|(i, _)| *i).collect()
    }

    /// Neue Profil-ID, die auch von keiner Sitzung, keinem Projekt/Ordner und keiner Vorlage mehr
    /// referenziert wird – sonst erbten diese still ein neues, fremdes Profil (z. B. mit Admin-Rechten).
    fn next_profile_id(&self) -> u64 {
        let mut m = self.settings.profiles.iter().map(|p| p.id).max().unwrap_or(0);
        m = m.max(self.entries.values().map(|e| e.meta.profile).max().unwrap_or(0));
        for p in &self.projects {
            m = m.max(p.profile);
            m = m.max(p.folders.iter().map(|f| f.profile).max().unwrap_or(0));
        }
        for t in &self.templates {
            m = m.max(t.items.iter().map(|i| i.profile).max().unwrap_or(0));
        }
        m.saturating_add(1)
    }

    /// Sitzungen des Projekts, die nicht abgedockt sind (dürfen ins Hauptlayout).
    fn docked_ids(&self) -> Vec<u64> {
        self.project_ids().into_iter().filter(|i| !self.detached.contains(i)).collect()
    }

    /// Wie `docked_ids`, ohne minimierte Sitzungen (die kommen nur zurück, wenn man sie holt).
    fn unminimized_docked(&self) -> Vec<u64> {
        self.docked_ids().into_iter().filter(|i| !self.is_minimized(*i)).collect()
    }

    fn is_minimized(&self, id: u64) -> bool {
        self.entries.get(&id).is_some_and(|e| e.meta.minimized)
    }

    fn switch_project(&mut self, id: u64) {
        if id == self.active_project {
            return;
        }
        let Some(p) = self.projects.iter().find(|p| p.id == id).cloned() else { return };
        let (layout, focused) = (self.layout.take(), self.focused);
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == self.active_project) {
            p.layout = layout;
            p.focused = focused;
        }
        self.active_project = id;
        self.load_project_view(p.layout, p.focused);
        self.procs_project_opened(id);
    }

    /// Zeile der Sitzung in der Seitenleiste sichtbar machen: Filter leeren, wenn er sie verbirgt, eingeklappte Ordner
    /// darüber aufklappen (die Gruppen der Listenansicht öffnet die Seitenleiste selbst, solange umbenannt wird).
    /// `false`, wenn sie hier nicht erscheinen kann (anderes Fenster, anderes Projekt ohne Filter, unbekannt).
    fn reveal_row(&mut self, id: u64) -> bool {
        let Some(e) = self.entries.get(&id) else { return false };
        if !self.in_win(e) {
            return false;
        }
        if !self.sidebar_match(e) {
            self.filter.clear();
        }
        let Some(e) = self.entries.get(&id) else { return false };
        if !self.sidebar_match(e) {
            return false;
        }
        let (project, mut path) = (e.meta.project, e.meta.group.clone());
        if self.filter.trim().is_empty() {
            if let Some(p) = self.projects.iter_mut().find(|p| p.id == project) {
                while !path.is_empty() {
                    if let Some(f) = p.folders.iter_mut().find(|f| f.path == path) {
                        f.collapsed = false;
                    }
                    path = state::folder_parent(&path).to_string();
                }
            }
        }
        true
    }

    fn sidebar_match(&self, e: &Entry) -> bool {
        if !self.in_win(e) {
            return false;
        }
        let f = self.filter.trim().to_lowercase();
        if f.is_empty() {
            return e.meta.project == self.active_project;
        }
        if let Some(t) = f.strip_prefix('#') {
            return e.meta.tags.iter().any(|x| x.to_lowercase().contains(t));
        }
        e.display_name().to_lowercase().contains(&f) || e.meta.tags.iter().any(|x| x.to_lowercase().contains(&f))
    }

    fn profile(&self, id: u64) -> Option<&Profile> {
        if id == 0 { None } else { self.settings.profiles.iter().find(|p| p.id == id) }
    }

    /// Shell-Kennung, die eine neue Sitzung mit diesem Profil ohne ausdrückliche Shell bekommt (wie `new_session`).
    fn shell_for_profile(&self, pid: u64) -> String {
        match self.profile(pid) {
            Some(p) if !p.program.trim().is_empty() => format!("custom:{}", p.id),
            Some(p) if !p.shell_id.is_empty() => p.shell_id.clone(),
            _ => self.settings.default_shell.clone(),
        }
    }

    /// Shell, mit der `new_session` tatsächlich startet: ein Standardprofil des Projekts mit eigenem Programm gewinnt,
    /// sonst die gewünschte Shell, sonst die des Profils bzw. die Standard-Shell.
    fn effective_shell_id(&self, shell: Option<&str>, project: u64) -> String {
        let pid = self.projects.iter().find(|p| p.id == project).map(|p| p.profile).unwrap_or(0);
        match self.profile(pid) {
            Some(p) if !p.program.trim().is_empty() => format!("custom:{}", p.id),
            _ => shell.map(str::to_string).unwrap_or_else(|| self.shell_for_profile(pid)),
        }
    }

    /// Ziel des Ordnerfelds für Ordner-/Projektvorgaben mit diesem Profil (WSL-Shell → Linux-Pfad, #172).
    fn folder_target(&self, pid: u64) -> folder_picker::Target {
        folder_picker::Target::for_shell(self.shell_by_id(&self.shell_for_profile(pid)).as_ref(), false)
    }

    /// Ziel des Ordnerfelds im Dialog „Neue Sitzung“: die gewählte Shell (bzw. die des Profils); bei einer erkannten
    /// WSL-Shell ist die Distribution dort wählbar (wechselt die Shell).
    fn new_dialog_target(&self, shell_id: &str, profile: u64) -> folder_picker::Target {
        let sid = match self.profile(profile) {
            Some(p) if !p.program.trim().is_empty() => format!("custom:{}", p.id),
            _ => shell_id.to_string(),
        };
        let choose = sid.starts_with("wsl:") && self.shells.iter().any(|s| s.id == sid);
        folder_picker::Target::for_shell(self.shell_by_id(&sid).as_ref(), choose)
    }

    /// Shell für eine Konversation aus dem (Windows-)Verlauf: `None` = wie gewohnt (Standard-Shell bzw. Profil des
    /// Projekts, beides Windows); sonst eine Windows-Shell, wenn Standard-Shell oder Projektprofil in WSL laufen.
    fn windows_shell_id(&self) -> Option<String> {
        let profile = self.projects.iter().find(|p| p.id == self.active_project).map(|p| p.profile).unwrap_or(0);
        let sid = match self.profile(profile) {
            Some(p) if !p.program.trim().is_empty() => format!("custom:{}", p.id),
            Some(p) if !p.shell_id.is_empty() => p.shell_id.clone(),
            _ => self.settings.default_shell.clone(),
        };
        if !self.shell_by_id(&sid).is_some_and(|s| shells::is_wsl(&s)) {
            return None;
        }
        self.shells.iter().find(|s| !shells::is_wsl(s)).map(|s| s.id.clone())
    }

    /// Stand der globalen Hooks neu lesen (nur lesend): eingetragen? Auf eine andere paneldeck.exe?
    fn refresh_hooks_global(&mut self) {
        let path = hooks::global_settings_path();
        self.hooks_global = path.as_deref().is_some_and(hooks::global_installed);
        self.hooks_foreign = if self.hooks_global { path.as_deref().and_then(|p| hooks::global_foreign_exe(p, &self.exe)) } else { None };
        self.hooks_incomplete = self.hooks_global && path.as_deref().is_some_and(hooks::global_incomplete);
    }

    /// Hat ein Terminal den Tastaturfokus?
    fn terminal_has_focus(&self, ctx: &egui::Context) -> bool {
        ctx.memory(|m| m.focused()).is_some_and(|f| self.entries.keys().any(|id| Id::new(("term", *id)) == f))
    }

    /// Fokus eines Terminals abgeben (ein Feld eines Dialogs behält ihn).
    fn release_terminal_focus(&self, ctx: &egui::Context) {
        if self.terminal_has_focus(ctx) {
            models_ui::release_focus(ctx);
        }
    }

    /// Erkannte WSL-Shell einer Distribution (Groß-/Kleinschreibung egal, wie Windows).
    fn wsl_shell_for(&self, distro: &str) -> Option<String> {
        self.shells.iter().find(|s| s.id.strip_prefix("wsl:").is_some_and(|d| d.eq_ignore_ascii_case(distro))).map(|s| s.id.clone())
    }

    fn shell_by_id(&self, id: &str) -> Option<ShellSpec> {
        if let Some(pid) = id.strip_prefix("custom:").and_then(|n| n.parse::<u64>().ok()) {
            if let Some(p) = self.profile(pid) {
                let args = split_args(&p.args);
                return Some(ShellSpec { id: custom_program_shell_id(p), label: p.name.clone(), program: p.program.clone(), args });
            }
        }
        self.shells
            .iter()
            .find(|s| s.id == id)
            .or_else(|| self.shells.iter().find(|s| s.id == self.settings.default_shell))
            .or_else(|| self.shells.first())
            .cloned()
    }

    fn scrollback_path(&self, id: u64) -> PathBuf {
        self.dir.join("scrollback").join(format!("{id}.txt"))
    }

    /// Shell einer Sitzung, wie sie beim Start aufgelöst wird (Profil mit eigenem Programm, sonst die Shell bzw. die
    /// Standard-Shell) – für die Vorschau der Kommandozeile in den Dialogen (#292).
    fn preview_spec(&self, shell_id: &str, profile: u64) -> Option<ShellSpec> {
        if let Some(p) = self.profile(profile).filter(|p| !p.program.trim().is_empty()) {
            return Some(ShellSpec { id: custom_program_shell_id(p), label: p.name.clone(), program: p.program.clone(), args: split_args(&p.args) });
        }
        self.session_shell(shell_id, profile)
    }

    /// Shell einer Sitzung mit der Shell-Wahl ihres Profils (#57, „Shell in WSL“: `-e <pfad> -l` statt der Login-Shell).
    fn session_shell(&self, shell_id: &str, profile: u64) -> Option<ShellSpec> {
        let mut s = self.shell_by_id(shell_id)?;
        if let Some(p) = self.profile(profile).filter(|p| p.program.trim().is_empty() && !p.login_shell.is_empty()) {
            if shells::is_wsl(&s) && !shells::is_system_bash(&s.program) {
                crate::shellinit::apply_login_override(&mut s.args, &p.login_shell);
            }
        }
        Some(s)
    }

    /// Vorbelegung der Claude-Optionen im Dialog „Neue Sitzung“: letzte Wahl, sonst die Standards der Einstellungen.
    fn dialog_claude_opts(&self) -> ClaudeOpts {
        self.settings.last_claude_opts.clone().unwrap_or_else(|| self.settings.claude_defaults.clone()).sanitized(false)
    }

    /// Befehlszeile für Claude; zweiter Wert: eine neue Konversation beginnt; dritter: die tatsächlich gesetzten
    /// Optionen und die weggelassenen Pfade (#182).
    fn claude_command(&self, meta: &mut SessionMeta, l: &Launch) -> (String, bool, crate::claude_opts::LaunchArgs, hooks::ConvBinding) {
        let spec = self.session_shell(&meta.shell_id, meta.profile);
        // Quoting nach der laufenden Shell (#57: in WSL die erkannte Login-Shell – zsh, fish …).
        let kind = spec.as_ref().map(shells::quote_kind).unwrap_or(crate::quote::Kind::Posix);
        let sid = spec.as_ref().map(|s| s.id.clone()).unwrap_or_else(|| meta.shell_id.clone());
        let is_wsl = shell_is_wsl(spec.as_ref(), &meta.shell_id);
        // Pfade der Optionen nach der Art der Shell (auch eigene Profile mit wsl.exe/bash.exe); Git Bash ohne MSYS-Umschreibung.
        let style = crate::claude_opts::PathStyle::for_shell(&sid, spec.as_ref().map(|s| s.program.as_str()).unwrap_or(""), kind);
        let mut c = String::from(style.claude_program());
        if is_wsl {
            // WSL: eigene Hook-Datei mit WSL-Pfad zur .exe (Aufruf über Interop); die globale Windows-Konfiguration gilt dort nicht.
            if let Some(f) = hooks::write_session_settings_wsl(&self.dir, &self.exe, &sid) {
                c += &format!(" --settings {}", crate::quote::quote(kind, &pathfmt::convert(&sid, &f.to_string_lossy())));
            }
        } else if hooks::session_needs_settings(
            self.hooks_global,
            self.hooks_incomplete,
            self.hooks_foreign.is_some(),
            &self.env_lines_of(meta),
            hooks::global_settings_path().as_deref(),
        ) {
            // Ohne globale Hooks immer; mit ihnen, wenn das Profil ein anderes CLAUDE_CONFIG_DIR setzt (dort gelten sie
            // nicht) oder ihnen Ereignisse fehlen und sie dieselbe paneldeck.exe aufrufen (`session_needs_settings`).
            if let Some(f) = &self.hooks_file {
                c += &format!(" --settings {}", crate::quote::quote(kind, &f.to_string_lossy().replace('\\', "/")));
            }
        }
        // Gespeicherte Optionen nur in harmloser Form übernehmen (state.json ist eine Vertrauensgrenze):
        // geprüfte Startoptionen (#161, #182; Pfade erst hier auf Existenz geprüft und für WSL umgerechnet), gequotet für
        // die Shell, dazu ältere freie `claude_args` bereinigt (vor den mehrwertigen Wertoptionen).
        let (opts, args) = meta.claude_opts_eff();
        let launch = opts.launch(kind, style, &args);
        c += &launch.line;
        // Fortsetzen folgt „Fortsetzen nach Neustart“ der Sitzung (#302) und beim Programmstart zusätzlich der globalen
        // Einstellung (`session_settings::resume_wanted`): „Fortsetzen“ = `--continue`, aber nur, wenn es im Ordner schon
        // eine Konversation gibt (sonst schlüge es fehl); „Konversation auswählen“ = die vorher gewählte (`resume_id`).
        // Der erste Start einer neuen Sitzung beginnt immer neu, außer der Verlauf gibt eine ID vor oder „letzte im Ordner“.
        let start = session_settings::launch_start(l.restore, l.restart, l.resume_id, l.continue_last, self.settings.claude_resume, meta, |m| {
            session_settings::folder_has_conversation(m, is_wsl, crate::history::projects_dir().as_deref(), &home_dir())
        });
        let fresh = start == session_settings::ClaudeStart::Fresh;
        let expect = match start {
            session_settings::ClaudeStart::Resume(id) => {
                c += &format!(" --resume {id}");
                Some(id)
            }
            session_settings::ClaudeStart::Continue => {
                c += " --continue";
                None
            }
            session_settings::ClaudeStart::Fresh => {
                // Neue Konversation ohne eigene ID (#302): nur `claude` mit den Hook-Einstellungen. Der Sitzung gehört sie
                // über den ersten SessionStart des gestarteten Claude (`hooks::ConvBinding`, wie bei `--continue`).
                meta.claude_session.clear();
                meta.claude_started = false;
                None
            }
        };
        // Der Start-Prompt wird nicht in die Kommandozeile geschrieben (Shell-Expansion von $, `, % …),
        // sondern als erster Eintrag der Warteschlange gesendet, sobald Claude bereit ist.
        if let Some(p) = l.prompt.filter(|p| !p.trim().is_empty()) {
            meta.queue.insert(0, p.trim().to_string());
        }
        // Zuordnung der Hook-Meldungen dieses Starts (#260): die übergebene Konversations-ID und ob `--safe-mode`
        // (Option oder alte `claude_args`) die Hooks des Claude dieser Sitzung abschaltet.
        let hooks_off = launch.applied.extra.flags.iter().any(|f| f == "safe-mode") || args.split_whitespace().any(|t| t == "--safe-mode");
        let binding = hooks::ConvBinding::launch(expect, hooks_off);
        (c + "\r", fresh, launch, binding)
    }

    fn spawn_into(&self, ctx: &egui::Context, entry: &mut Entry, l: Launch) {
        // Ab hier gilt die Einstellung für diesen Start (auch wenn der Start gleich scheitert: dann läuft nichts mehr
        // aus der alten Konfiguration).
        // Mit ausgeschaltetem Claude-Modus startet keine Sitzung Claude – auch nicht beim Programmstart, Neu laden oder
        // Duplizieren (#261); die Einstellung der Sitzung bleibt für später erhalten.
        entry.launched_claude = entry.meta.claude && !entry.is_chat() && self.settings.claude_mode;
        entry.launched_opts = if entry.launched_claude { entry.meta.claude_opts_eff().0 } else { Default::default() };
        entry.hook_seen = false;
        entry.pick_pending = false;
        entry.hook_conv = hooks::ConvBinding::default();
        entry.hook_keyed = false;
        // Die Warteschlange hatte der Hub selbst angehalten, weil Claude beendet war: Mit dem neuen Start läuft sie wieder.
        if entry.track.auto_paused {
            entry.meta.queue_paused = false;
        }
        entry.track = Default::default();
        entry.clear_status_msg();
        entry.perm_since = None;
        let Some(mut shell) = self.session_shell(&entry.meta.shell_id, entry.meta.profile) else {
            entry.error = Some(state::t("Keine Shell gefunden", "No shell found").into());
            return;
        };
        let wsl = shells::is_wsl(&shell);
        // Ob im Vordergrund der Shell etwas läuft, sieht der Hub nur bei lokalen Shells (nicht in WSL).
        entry.track.confirmable = shells::foreground_visible(&shell);
        let restore_text = if l.restore {
            std::fs::read_to_string(self.scrollback_path(entry.meta.id)).ok()
        } else {
            None
        };
        // WSL: ein Linux-Startordner (leer = Linux-Home `~`, #172) geht als `wsl.exe --cd <pfad>` mit; der
        // Windows-Startordner ist dann nur ein gültiger Platzhalter. Windows-Pfade übersetzt wsl.exe wie bisher selbst.
        // Ein gespeicherter `\\wsl$\<distro>\…`-Ordner derselben Distribution geht ebenfalls über `--cd`; Pfade mit `"`/`\`
        // erreichen die Kommandozeile nie (wsl.exe zerlegt sie selbst – Befehlseinschleusung).
        let cwd = if wsl {
            // Ohne `-d`: die bekannte Standard-Distribution (ein `\\wsl.localhost\<D>\…`-Ordner derselben geht dann über `--cd`).
            let distro = wsl_distro_of(&shell).or_else(crate::wsl::cached_default);
            // Home und Einhängepunkt für spätere Umrechnungen (Links, Einfügen, `~`) im Hintergrund holen (#392).
            crate::wsl::prefetch(distro.as_deref());
            let de = self.settings.language == Lang::De;
            // Der WSL-Starter `bash.exe` kennt kein `--cd`: er startet im Windows-Ordner (#261).
            let cd = if shells::is_system_bash(&shell.program) { crate::wsl::Cd::Windows } else { crate::wsl::apply_cd(&mut shell.args, &entry.meta.cwd, distro.as_deref()) };
            match cd {
                crate::wsl::Cd::Linux => home_dir(),
                // Eigenes `--cd`/`~` des Profils (oder keine bekannte Distribution): nie einen Linux- oder WSL-Pfad als
                // Windows-Startordner (CreateProcess) – und keine Prüfung eines \\wsl$-Pfads im UI-Thread.
                crate::wsl::Cd::Windows if spawn_cwd_is_windows(&entry.meta.cwd) => entry.meta.cwd.clone(),
                crate::wsl::Cd::Windows => home_dir(),
                crate::wsl::Cd::OtherDistro(d) => {
                    let msg = if de {
                        format!("Der Startordner liegt in der WSL-Distribution {d}, die Shell startet eine andere: {}", entry.meta.cwd)
                    } else {
                        format!("The start folder is in the WSL distribution {d}, but the shell starts another one: {}", entry.meta.cwd)
                    };
                    entry.error = Some(msg);
                    entry.live = None;
                    return;
                }
                crate::wsl::Cd::Invalid(e) => {
                    // Gespeicherter Ordner (Wiederherstellen, Teilen, Duplizieren; kann per OSC 7 vom Terminalinhalt stammen)
                    // ist als Startordner unzulässig: im Linux-Home starten und das melden, statt die Sitzung zu verweigern.
                    // Dialog und MCP lehnen solche Pfade schon vorher ab.
                    let old = std::mem::replace(&mut entry.meta.cwd, "~".into());
                    // Die Konversation gehört zum alten Ordner: nicht im Linux-Home mit `--resume` suchen (#245).
                    entry.meta.claude_started = false;
                    let _ = crate::wsl::apply_cd(&mut shell.args, "~", distro.as_deref());
                    self.spawn_notes.borrow_mut().push(if de {
                        format!("Startordner nicht verwendbar ({}): {} – die Sitzung startet im Linux-Home (~).", e.message(true), crate::mcp::short(&old, 80))
                    } else {
                        format!("Start folder not usable ({}): {} – the session starts in the Linux home (~).", e.message(false), crate::mcp::short(&old, 80))
                    });
                    home_dir()
                }
            }
        } else if entry.meta.cwd.is_empty() {
            home_dir()
        } else if cfg!(windows) && (entry.meta.cwd.starts_with('/') || entry.meta.cwd.starts_with('~')) {
            // (Linux/macOS, #149: Unix-Pfade sind dort die normalen Pfade.)
            // Nie einen Linux-Pfad an eine Windows-Shell geben (#392): Git Bash `/c/x`, `/mnt/c/x` → `C:\x`, `~`/`~/x` =
            // Windows-Benutzerordner; was Windows nicht kennt, startet im Windows-Home.
            crate::targetpath::PathKind::of(&shell).to_windows(&entry.meta.cwd).unwrap_or_else(|_| home_dir())
        } else {
            entry.meta.cwd.clone()
        };
        // cmd.exe kennt keinen UNC-Arbeitsordner (startet sonst in C:\Windows, #392): im Benutzerordner starten und sagen, warum.
        let cwd = if !wsl && matches!(crate::targetpath::PathKind::of(&shell), crate::targetpath::PathKind::Windows { cmd: true }) && (cwd.starts_with("\\\\") || cwd.starts_with("//")) {
            let de = self.settings.language == Lang::De;
            self.spawn_notes.borrow_mut().push(if de {
                format!("cmd.exe kann nicht in einem Netzwerk- oder WSL-Ordner starten ({}) – die Sitzung startet im Benutzerordner.", crate::mcp::short(&cwd, 80))
            } else {
                format!("cmd.exe cannot start in a network or WSL folder ({}) – the session starts in the home folder.", crate::mcp::short(&cwd, 80))
            });
            home_dir()
        } else {
            cwd
        };
        // Erst nach dem Startordner: ein unbrauchbarer gespeicherter Ordner setzt die Konversation zurück (#245).
        let claude_cmd = if entry.launched_claude {
            // Alte freie Argumente einmalig umstellen; was nicht mehr geht (z. B. `--resume`, `--print`), meldet ein
            // Hinweis – einmal, danach steht es nicht mehr in der Sitzung.
            let legacy_dropped = entry.meta.migrate_legacy_args();
            let (c, fresh, launch, binding) = self.claude_command(&mut entry.meta, &l);
            entry.hook_conv = binding;
            // Auskünfte über MCP nennen nur, was wirklich gesetzt ist; weggelassene Pfade meldet ein Hinweis.
            entry.launched_opts = launch.applied;
            let mut notes = Vec::new();
            if !legacy_dropped.is_empty() {
                notes.push(legacy_dropped_note(&entry.display_name(), &legacy_dropped));
            }
            if !launch.dropped.is_empty() {
                notes.push(format!(
                    "{} ({}): {}",
                    state::t("Claude-Optionen nicht gefunden oder nicht lokal, weggelassen", "Claude options not found or not local, left out"),
                    entry.display_name(),
                    launch.dropped.join(", "),
                ));
            }
            if !notes.is_empty() {
                entry.launch_note = Some(notes.join("\n"));
            }
            if fresh {
                // Neue Konversation: altes Transkript und dessen Token/Kosten gehören nicht mehr dazu.
                entry.meta.transcript.clear();
                self.usage.reset(entry.meta.id);
            }
            // Zu lang für cmd (claude.cmd läuft darüber, #289): Claude nicht starten, sondern sagen, warum.
            let len = c.trim_end().chars().count();
            if len > crate::claude_opts::MAX_COMMAND_LEN {
                let msg = format!(
                    "{} ({}): {len} > {}",
                    state::t(
                        "Claude nicht gestartet – die Befehlszeile ist zu lang (System-Prompt oder Listen in den Claude-Optionen kürzen)",
                        "Claude not started – the command line is too long (shorten the system prompt or the lists in the Claude options)",
                    ),
                    entry.display_name(),
                    crate::claude_opts::MAX_COMMAND_LEN,
                );
                entry.launch_note = Some(match entry.launch_note.take() {
                    Some(n) => format!("{n}\n{msg}"),
                    None => msg,
                });
                entry.launched_claude = false;
                entry.launched_opts = Default::default();
                None
            } else {
                Some(c)
            }
        } else {
            self.usage.reset(entry.meta.id);
            None
        };
        let profile = self.profile(entry.meta.profile).cloned();
        let startup = profile.as_ref().map(|p| p.startup.trim().to_string()).filter(|s| !s.is_empty());
        let mut initial = match (&startup, claude_cmd) {
            (Some(st), Some(c)) => Some(format!("{st}\r{c}")),
            (Some(st), None) => Some(format!("{st}\r")),
            (None, c) => c,
        };
        // Schlüssel an zweiter Stelle: er landet so auch in WSLENV (siehe unten, alles ab Index 1 wird durchgereicht).
        entry.session_key = hooks::random_hex();
        let mut env = vec![(hooks::ENV_SESSION.to_string(), entry.meta.id.to_string()), (hooks::ENV_SESSION_KEY.to_string(), entry.session_key.clone())];
        if let Some(p) = &profile {
            if p.admin && l.restore {
                // Beim Programmstart (auch Autostart im Tray) nicht für jede gespeicherte Admin-Sitzung eine UAC-Abfrage öffnen.
                entry.error = Some(self.settings.language.t(
                    "Administrator-Sitzung – über „Neu starten“ öffnen (UAC-Abfrage).",
                    "Administrator session – open via “Restart” (UAC prompt).",
                ).to_string());
                entry.live = None;
                return;
            }
            if p.admin {
                // Erhöhte Rechte: eine ConPTY kann nicht in einen höheren Sicherheitskontext eingebettet werden;
                // die Shell startet deshalb (nach UAC-Abfrage) in einem eigenen Fenster.
                let msg = launch_elevated(&shell, &cwd);
                entry.error = Some(msg);
                entry.live = None;
                return;
            }
        }
        // Umgebungsvariablen (#384): Profil < Sitzung (die Vorlage ist beim Anlegen in die Sitzung kopiert). Geschützte
        // Namen (`PANELDECK_*`, intern) fallen weg – ein Profil überschreibt `PANELDECK_SESSION`/`_KEY` nicht mehr.
        // Lässt sich ein geheimer Wert nicht entschlüsseln, wartet die Sitzung („Entsperren und starten“, wie
        // Administrator-Sitzungen beim Programmstart).
        let user_vars = crate::session_env::merge(&profile.as_ref().map(|p| crate::session_env::from_profile(&p.env)).unwrap_or_default(), &entry.meta.env);
        let mut user_ops = match crate::session_env::resolve(&user_vars, crate::llm::secret::unprotect) {
            Ok(ops) => ops,
            Err(locked) => {
                entry.error = Some(env_locked_text(&locked, self.settings.language));
                entry.env_locked = Some(locked);
                entry.live = None;
                return;
            }
        };
        entry.env_locked = None;
        entry.env_pending = false;
        entry.env_live = entry.meta.env.clone();
        // WSL: nur gesetzte Werte gehen über WSLENV; Voranstellen/Anhängen/Entfernen wirken auf die Linux-Werte und gehen
        // deshalb als sichtbarer Befehl (führendes Leerzeichen) vor den Startbefehl – geheime nie, die fehlen mit Hinweis.
        let mut wsl_live: Vec<crate::session_env::Op> = Vec::new();
        if wsl {
            let (set, rest): (Vec<_>, Vec<_>) = user_ops.into_iter().partition(|o| matches!(o.kind, crate::session_env::OpKind::Set(_)));
            user_ops = set;
            let (secret, normal): (Vec<_>, Vec<_>) = rest.into_iter().partition(|o| o.secret);
            wsl_live = normal;
            if !secret.is_empty() {
                let names: Vec<String> = secret.iter().map(|o| o.name.clone()).collect();
                self.spawn_notes.borrow_mut().push(format!(
                    "{} ({}): {}",
                    state::t("Geheime Variablen zum Voranstellen/Anhängen/Entfernen gehen in WSL nicht, weggelassen", "Secret variables to prepend/append/remove do not work in WSL, left out"),
                    entry.display_name(),
                    names.join(", ")
                ));
            }
        }
        // Ein WSLENV des Nutzers wird zusammengeführt (Paneldecks Einträge kommen dazu), nicht ersetzt.
        let user_wslenv = user_ops.iter().rev().find(|o| o.name.eq_ignore_ascii_case("WSLENV")).and_then(|o| match &o.kind {
            crate::session_env::OpKind::Set(v) => Some(v.clone()),
            _ => None,
        });
        if let Some(h) = &self.hook_server {
            env.push((hooks::ENV_PORT.to_string(), h.port.to_string()));
            env.push((hooks::ENV_TOKEN.to_string(), h.token.clone()));
            // Datenordner dieses Hubs: `paneldeck.exe mcp` aus diesem Terminal findet damit genau diese Instanz
            // (auch bei --data-dir/--profile). Kein Geheimnis – den IPC-Token liest die Brücke aus `ipc.json`.
            env.push((crate::paths::ENV_HUB_DATA.to_string(), self.dir.display().to_string()));
            env.push((crate::paths::ENV_HUB_BASE.to_string(), crate::paths::get().base.display().to_string()));
        }
        if wsl {
            // Variablen an WSL durchreichen (und zurück an die Windows-.exe beim Hook-Aufruf); dazu die Variablen von
            // Profil und Sitzung (je mit `/p` bzw. `/l`, wenn „Pfad umrechnen“ gewählt ist).
            // Dazu Name/Version des Terminals (`terminal::term_env`), wie Windows Terminal es mit WT_SESSION macht.
            let mut names = vec![
                hooks::ENV_SESSION.to_string(),
                hooks::ENV_PORT.to_string(),
                hooks::ENV_TOKEN.to_string(),
                "TERM_PROGRAM".to_string(),
                "TERM_PROGRAM_VERSION".to_string(),
            ];
            for (k, _) in env.iter().skip(1) {
                if k != hooks::ENV_PORT && k != hooks::ENV_TOKEN && !names.contains(k) {
                    names.push(k.clone());
                }
            }
            let ours = names.join(":");
            let base = user_wslenv.clone().or_else(|| std::env::var("WSLENV").ok()).unwrap_or_default();
            let mut wslenv = if base.is_empty() { ours } else { format!("{base}:{ours}") };
            for op in user_ops.iter().filter(|o| !o.name.eq_ignore_ascii_case("WSLENV")) {
                wslenv = wslenv_with(&wslenv, &op.name, crate::session_env::wslenv_flag(op));
            }
            env.push(("WSLENV".to_string(), wslenv));
        }
        entry.shell_eff = shell.id.clone();
        entry.status = ClaudeStatus::Unknown;
        if self.settings.shell_integration {
            // zsh/fish (#57): Hilfsdateien im Datenordner; ein ZDOTDIR aus Profil oder Sitzung bleibt das des Nutzers.
            let mut known: Vec<(String, String)> = user_ops
                .iter()
                .filter_map(|o| match &o.kind {
                    crate::session_env::OpKind::Set(v) => Some((o.name.clone(), v.clone())),
                    _ => None,
                })
                .collect();
            known.extend(env.iter().cloned());
            let plan = shells::integration(&shell, &self.dir, &crate::shellinit::EnvCtx { env: &known, inherit: self.settings.inherit_env });
            entry.shell_flavor = plan.flavor;
            shell.args.extend(plan.args);
            for (k, v) in plan.env {
                if wsl {
                    // Variable an WSL durchreichen (Pfade mit /p: WSL rechnet sie nach der eigenen Einhänge-Konfiguration
                    // um). Das zuletzt gesetzte WSLENV gilt – ein WSLENV aus dem Profil steht davor.
                    let flag = if plan.path_vars.contains(&k) { "/p" } else { "" };
                    if let Some(w) = env.iter_mut().rev().find(|(n, _)| n == "WSLENV") {
                        w.1 = wslenv_with(&w.1, &k, flag);
                    }
                }
                env.push((k, v));
            }
        } else {
            entry.shell_flavor = None;
        }
        // Voranstellen/Anhängen/Entfernen in WSL (#384 Review): in der Syntax der erkannten Login-Shell (fish eigene);
        // ohne erkannte Shell oder für nicht wörtlich setzbare Schritte nicht – mit Hinweis, nie still.
        if !wsl_live.is_empty() {
            let lang = self.settings.language;
            let mut left: Vec<String> = Vec::new();
            match entry.shell_flavor.map(crate::session_env::LiveShell::of) {
                None => left.extend(wsl_live.iter().map(|o| o.name.clone())),
                Some(sh) => {
                    for o in &wsl_live {
                        if let Some(i) = crate::session_env::live_issue(sh, true, o) {
                            left.push(format!("{} ({})", o.name, i.text(lang)));
                        }
                    }
                    if let Some(cmd) = crate::session_env::live_command(sh, true, &wsl_live) {
                        initial = Some(format!("{cmd}\r{}", initial.unwrap_or_default()));
                    }
                }
            }
            if !left.is_empty() {
                self.spawn_notes.borrow_mut().push(format!(
                    "{} ({}): {}",
                    lang.t(
                        "Voranstellen/Anhängen/Entfernen in WSL braucht eine erkannte Shell (Shell-Integration) und wörtlich setzbare Werte – weggelassen",
                        "Prepend/append/remove in WSL needs a detected shell (shell integration) and values that can be set literally – left out",
                    ),
                    entry.display_name(),
                    left.join(", ")
                ));
            }
        }
        entry.track.launch_lines = claude_status::launch_lines(initial.as_deref());
        match Session::spawn(
            ctx,
            SpawnOptions {
                shell: &shell,
                cwd: Some(&cwd),
                restore_text: restore_text.as_deref(),
                initial_input: initial.as_deref(),
                scrollback_lines: self.settings.scrollback_lines,
                env,
                user_env: user_ops,
                // Farb- und Größenabfragen ab dem ersten Byte richtig beantworten (#183), auch ungezeichnet.
                view: Some(self.reply_view_for(ctx, entry.meta.profile)),
                inherit_env: self.settings.inherit_env,
            },
        ) {
            Ok(s) => {
                entry.live = Some(s);
                entry.error = None;
            }
            Err(e) => entry.error = Some(e),
        }
    }

    /// Legt eine neue Sitzung an und zeigt sie im fokussierten Bereich.
    fn new_session(&mut self, ctx: &egui::Context, mut o: NewOpts) -> u64 {
        let quiet = o.quiet;
        let chat_meta = o.chat.take();
        if chat_meta.is_some() {
            // Ein Chat hat weder Shell noch Profil noch Claude.
            o.profile = Some(0);
            o.claude = false;
        }
        let id = self.next_id;
        self.next_id += 1;
        let (window, project) = self.session_home(o.window, o.project);
        let pid = o.profile.unwrap_or_else(|| self.projects.iter().find(|p| p.id == project).map(|p| p.profile).unwrap_or(0));
        let prof = self.profile(pid).cloned();
        let shell_id = match &prof {
            Some(p) if !p.program.trim().is_empty() => format!("custom:{}", p.id),
            Some(p) if !p.shell_id.is_empty() && o.shell_id.is_none() => p.shell_id.clone(),
            _ => o.shell_id.clone().unwrap_or_else(|| self.settings.default_shell.clone()),
        };
        let o = NewOpts { cwd: o.cwd.filter(|c| !c.is_empty()).or_else(|| prof.as_ref().map(|p| p.cwd.clone()).filter(|c| !c.is_empty())), ..o };
        let label = self.shell_by_id(&shell_id).map(|s| s.label).unwrap_or_else(|| "Shell".into());
        // Alle Wege (Dialog, MCP, Kontextmenü, --claude, IPC, Vorlagen, Duplikat, Verlauf) laufen hier durch;
        // bypassPermissions nur bei ausdrücklicher Wahl im Dialog.
        let (claude_opts, claude_args, legacy_dropped) = if o.claude {
            crate::claude_opts::resolve_new_report(o.claude_opts.as_ref(), o.allow_bypass, &self.settings.claude_defaults, &o.claude_args)
        } else {
            (ClaudeOpts::default(), o.claude_args.clone(), Vec::new())
        };
        let (name, custom) = match o.name.filter(|n| !n.trim().is_empty()) {
            Some(n) => (n, true),
            None if chat_meta.is_some() => ("Chat".to_string(), false),
            None if o.claude => ("Claude".to_string(), false),
            None => (label, false),
        };
        let mut meta = SessionMeta {
            id,
            name,
            custom_name: custom,
            shell_id,
            cwd: o.cwd.filter(|c| !c.is_empty()).or_else(|| self.projects.iter().find(|p| p.id == project).map(|p| p.folder.clone())).unwrap_or_default(),
            project,
            color: session_color(o.color, prof.as_ref().and_then(|p| p.color), hub_color(id)),
            tags: o.tags.clone(),
            notes: o.notes.clone(),
            profile: pid,
            claude: o.claude,
            claude_args,
            claude_opts,
            claude_session: o.resume_id.clone().unwrap_or_default(),
            claude_started: o.resume_id.is_some(),
            transcript: o.transcript.clone(),
            group: o.group.as_deref().map(state::clean_folder_path).unwrap_or_default(),
            window,
            // Chats bekommen keine Umgebungsvariablen (#384).
            env: if chat_meta.is_some() { Vec::new() } else { crate::session_env::clean(o.env.clone()) },
            ..SessionMeta::default()
        };
        meta.set_resume_mode(o.resume_mode.unwrap_or_default());
        if !meta.group.is_empty() {
            let g = meta.group.clone();
            if let Some(pr) = self.projects.iter_mut().find(|p| p.id == meta.project) {
                pr.ensure_folder(&g);
            }
        }
        let entry = match chat_meta {
            Some(cm) => {
                // Liegt unter dieser Id noch ein fremder Verlauf, wird er beiseitegelegt, nie geladen oder überschrieben.
                let _ = chatmod::store::set_aside(&chatmod::store::path(&self.dir, id));
                // Ein Linux-Ordner (aus einer WSL-Sitzung, Ordnervorgabe, Projekt) wird zum Windows-Pfad
                // `\\wsl.localhost\<D>\…` – die Datei-Werkzeuge des Chats arbeiten über den Windows-Zugriff (#392).
                let cwd = self.chat_root(&meta.cwd, meta.project);
                let meta = SessionMeta { kind: SessionKind::Chat, chat: Some(cm), shell_id: String::new(), cwd, ..meta };
                let mut chat = ChatState::new(chatmod::ChatFile::default(), None);
                chat.focus_input = true;
                Entry::new_chat(meta, chat)
            }
            None => {
                let mut entry = Entry::new(meta);
                self.spawn_into(
                    ctx,
                    &mut entry,
                    Launch { restore: false, restart: false, continue_last: o.continue_last, prompt: o.prompt.as_deref(), resume_id: o.resume_id.as_deref() },
                );
                // Aus alten Argumenten (Vorlage, Duplikat) weggelassene Flags melden (Review zu #245).
                if !legacy_dropped.is_empty() {
                    let n = legacy_dropped_note(&entry.display_name(), &legacy_dropped);
                    entry.launch_note = Some(match entry.launch_note.take() {
                        Some(old) => format!("{n}\n{old}"),
                        None => n,
                    });
                }
                entry
            }
        };
        self.entries.insert(id, entry);
        if !quiet {
            if window == self.wins.cur {
                // Gehört die neue Sitzung zu einem anderen Projekt (z. B. „Duplizieren“ einer gefilterten Sitzung), erst dorthin
                // wechseln – sonst landete sie im Layout des gezeigten Projekts und verschwände beim nächsten Wechsel (#258).
                self.switch_project(project);
                self.activate(id);
            } else {
                // Anderes Fenster: dort zeigen (es kommt nach vorn), nicht im gerade gezeichneten Layout.
                self.route_to(ctx, window, Action::Activate(id));
            }
        }
        id
    }

    /// Alle laufenden Sitzungen eines Ordners als Raster neben/anstelle von `tid` öffnen (z: 0 ersetzen, 1 links, 2 rechts, 3 oben, 4 unten).
    fn open_folder_at(&mut self, folder: &str, tid: u64, z: u8) {
        let lang = self.settings.language;
        let mut ids: Vec<u64> = self.folder_sessions(folder).into_iter().filter(|id| self.entries.get(id).is_some_and(|e| !e.exited())).collect();
        ids.retain(|i| *i != tid);
        ids.truncate(12);
        let Some(sub) = Node::grid(&ids, Dir::Horizontal) else {
            self.notify_text(lang.t("Keine laufenden Sitzungen in diesem Ordner", "No running sessions in this folder").to_string());
            return;
        };
        let Some(mut layout) = self.layout.take() else { return };
        for id in &ids {
            if layout.contains(*id) {
                layout = layout.remove_leaf(*id).unwrap_or(Node::Leaf(tid));
            }
        }
        self.detached.retain(|d| !ids.contains(d));
        match z {
            0 => {
                layout.replace_leaf_with(tid, sub);
            }
            _ => {
                let (dir, before) = match z {
                    1 => (Dir::Horizontal, true),
                    2 => (Dir::Horizontal, false),
                    3 => (Dir::Vertical, true),
                    _ => (Dir::Vertical, false),
                };
                layout.split_leaf_with(tid, sub, dir, before);
            }
        }
        self.layout = Some(layout);
        self.focused = ids[0];
        self.want_focus = Some(ids[0]);
    }

    /// Benennt/verschiebt einen Ordner samt Unterordnern und Sitzungen. `false` bei Namenskonflikt oder Verschieben in sich selbst.
    fn rename_folder_to(&mut self, old: &str, new: &str) -> bool {
        let pid = self.active_project;
        let ok = self.projects.iter_mut().find(|p| p.id == pid).is_some_and(|p| p.rename_folder(old, new));
        if ok {
            for e in self.entries.values_mut() {
                if e.meta.project == pid && state::folder_within(&e.meta.group, old) {
                    e.meta.group = format!("{}{}", new, &e.meta.group[old.len()..]);
                }
            }
        }
        ok
    }

    fn active_folder_mut(&mut self, path: &str) -> Option<&mut state::Folder> {
        let pid = self.active_project;
        self.projects.iter_mut().find(|p| p.id == pid)?.folders.iter_mut().find(|f| f.path == path)
    }

    /// Sitzungen des aktiven Projekts in diesem Ordner (inkl. Unterordner), nach Id sortiert.
    fn folder_sessions(&self, path: &str) -> Vec<u64> {
        self.entries.iter().filter(|(_, e)| e.meta.project == self.active_project && self.in_win(e) && !e.meta.group.is_empty() && state::folder_within(&e.meta.group, path)).map(|(i, _)| *i).collect()
    }

    fn set_group(&mut self, id: u64, g: String) {
        let g = state::clean_folder_path(&g);
        let pid = match self.entries.get_mut(&id) {
            Some(e) => {
                e.meta.group = g.clone();
                e.meta.project
            }
            None => return,
        };
        if !g.is_empty() {
            if let Some(p) = self.projects.iter_mut().find(|p| p.id == pid) {
                p.ensure_folder(&g);
            }
        }
    }

    /// Sitzungen, deren Ordner noch nicht in der Ordnerliste ihres Projekts steht (alte Gruppen), nachtragen.
    fn sync_folders(&mut self) {
        for e in self.entries.values() {
            if e.meta.group.is_empty() {
                continue;
            }
            if let Some(p) = self.projects.iter_mut().find(|p| p.id == e.meta.project) {
                if p.folder(&e.meta.group).is_none() {
                    p.ensure_folder(&e.meta.group);
                }
            }
        }
    }

    /// Sitzung zeigen und fokussieren (Seitenleiste, Palette „Wechseln zu“, MCP `focus_session`, Strg+Tab …).
    /// Steht sie nicht im Layout, gilt (`layout::show`):
    /// 1. leerer Hub → sie wird der einzige Bereich;
    /// 2. minimiert → sie öffnet sich allein und ersetzt das ganze Layout dieses Fensters/Projekts; die bisher sichtbaren
    ///    Bereiche werden zu minimierten Sitzungen (sie laufen weiter, ein Klick holt jede genauso zurück);
    /// 3. sonst (nur weggewechselt, nie minimiert) → den fokussierten Bereich ersetzen, wie bisher.
    ///
    /// Eine Sitzung eines anderen Fensters kommt hier nicht an (`route_target` reicht die Aktion dorthin weiter, dort gilt
    /// dieselbe Regel); bei einer eines anderen Projekts wechselt `Action::Activate` vorher das Projekt.
    ///
    /// Werden dabei Bereiche minimiert, erscheint kurz der Hinweis „N Bereiche minimiert“ (#162).
    fn activate(&mut self, id: u64) {
        let Some(e) = self.entries.get_mut(&id) else { return };
        e.attention = false;
        // Eine Sitzung darf nie gleichzeitig abgedockt und im Hauptlayout sein (zwei Größen kämpften um die PTY).
        self.detached.retain(|d| *d != id);
        let n = show_session(&mut self.layout, &mut self.entries, id, self.focused);
        self.focused = id;
        self.want_focus = Some(id);
        if n > 0 {
            self.notify_text(minimized_hint(n, self.settings.language));
        }
    }

    /// Sitzung für einen Agenten platzieren (`Action::Place`, #162). Aktiver Bereich ist der Bereich der aufrufenden Sitzung,
    /// wenn er im Layout steht, sonst der fokussierte Bereich des Nutzers (`agent_anchor`). `Solo` minimiert die übrigen
    /// Bereiche (mit Hinweis), `Replace` ersetzt nur den aktiven Bereich (er wird minimiert), die Teilungen teilen nur ihn.
    /// `protected` wird nie verdrängt (`layout::place`).
    ///
    /// Entscheidung des Nutzers: Der Tastaturfokus bleibt, wo er war – außer der fokussierte Bereich hat das Layout verlassen
    /// (dann die neue Sitzung). Gehört die Sitzung zu einem anderen als dem gezeigten Projekt, wird dessen gespeichertes
    /// Layout geändert, ohne das Projekt zu wechseln.
    fn place(&mut self, id: u64, placement: crate::layout::Placement, caller: Option<u64>, protected: &[u64]) {
        let Some(e) = self.entries.get_mut(&id) else { return };
        e.attention = false;
        let pid = e.meta.project;
        self.detached.retain(|d| *d != id);
        let prot = |x: u64| protected.contains(&x);
        if pid != self.active_project {
            if let Some(p) = self.projects.iter_mut().find(|p| p.id == pid) {
                let anchor = agent_anchor(p.layout.as_ref(), caller, p.focused);
                place_session(&mut p.layout, &mut self.entries, id, anchor, placement, &prot);
                p.focused = keep_focus(p.layout.as_ref(), p.focused, id);
            }
            return;
        }
        let anchor = agent_anchor(self.layout.as_ref(), caller, self.focused);
        let n = place_session(&mut self.layout, &mut self.entries, id, anchor, placement, &prot);
        let focus = keep_focus(self.layout.as_ref(), self.focused, id);
        if focus != self.focused {
            self.focused = focus;
            self.want_focus = Some(focus);
        }
        if n > 0 {
            self.notify_text(minimized_hint(n, self.settings.language));
        }
    }

    /// Bereich minimieren: die Sitzung verlässt das Layout, läuft aber weiter und bleibt in der Seitenleiste (abgeblendet).
    /// Ihr Platz wird nicht gemerkt – zurückgeholt öffnet sie sich allein (`activate`). Auch der letzte Bereich darf gehen –
    /// dann bleibt der leere Hub. Ein abgedocktes Fenster schließt sich dabei, ohne die Sitzung wieder anzudocken.
    fn minimize_pane(&mut self, id: u64) {
        if !self.entries.contains_key(&id) {
            return;
        }
        if self.detached.contains(&id) {
            self.detached.retain(|d| *d != id);
        } else {
            let Some(focus) = crate::layout::minimize(&mut self.layout, id, self.focused) else { return }; // schon draußen
            if focus != self.focused {
                self.focused = focus;
                self.want_focus = Some(focus);
            }
        }
        if let Some(e) = self.entries.get_mut(&id) {
            e.meta.minimized = true;
            e.meta.slot = None; // veralteter Platz aus 2.2.0 (deprecated): wird nie neu angelegt
        }
    }

    fn split_at(&mut self, ctx: &egui::Context, target: u64, dir: Dir, before: bool) {
        let Some(src) = self.entries.get(&target) else { return };
        let (shell, cwd) = (src.meta.shell_id.clone(), src.cwd_now());
        // Fenster und Projekt der geteilten Sitzung (ein abgedockter Bereich eines weiteren Fensters wird im Hauptbild gezeichnet).
        let (window, project) = (src.meta.window, src.meta.project);
        let id = self.next_id;
        self.next_id += 1;
        let label = self.shell_by_id(&shell).map(|s| s.label).unwrap_or_else(|| "Shell".into());
        let meta = SessionMeta {
            id,
            name: label,
            custom_name: false,
            shell_id: shell,
            cwd,
            color: hub_color(id),
            group: self.entries[&target].meta.group.clone(),
            project,
            profile: self.entries[&target].meta.profile,
            window,
            // Teilen kopiert die Umgebungsvariablen (#384); danach sind sie unabhängig.
            env: self.entries[&target].meta.env.clone(),
            ..SessionMeta::default()
        };
        let mut entry = Entry::new(meta);
        self.spawn_into(ctx, &mut entry, Launch { restore: false, restart: false, continue_last: false, prompt: None, resume_id: None });
        self.entries.insert(id, entry);
        if window != self.wins.cur {
            return; // steht in der Seitenleiste seines Fensters; das hier gezeichnete Layout bleibt unberührt
        }
        if let Some(l) = &mut self.layout {
            l.split_leaf_at(target, id, dir, before);
        }
        self.focused = id;
        self.want_focus = Some(id);
    }

    fn hide_pane(&mut self, id: u64) {
        if let Some(l) = self.layout.take() {
            self.layout = l.remove_leaf(id);
        }
        if self.focused == id {
            // Fokus nie auf einer Sitzung außerhalb des Layouts lassen (auch nicht, wenn nichts mehr übrig ist).
            self.focused = self.layout.as_ref().map(|l| l.leaf_ids()[0]).unwrap_or(0);
            self.want_focus = Some(self.focused);
        }
    }

    fn close_session(&mut self, id: u64) {
        self.hide_pane(id);
        self.detached.retain(|d| *d != id);
        // Job mit übrigen Prozessen behalten (#218): Sie handeln weiter als diese Sitzung, mit ihrer Kette.
        let chain = taint::ancestors(&self.entries, Some(id));
        if let Some(mut e) = self.entries.remove(&id) {
            let name = e.display_name();
            if let Some(s) = e.live.as_mut() {
                self.retire_job(id, name, s);
            }
        }
        if self.retired.iter().any(|r| r.session == id) {
            taint::set_retired(id, chain);
        }
        self.usage.remove(id);
        self.git.remove(id);
        // Agenten-Zuordnung: auf jedem Weg (Knopf, Mittelklick, „alle schließen“, MCP) aufräumen.
        self.mcp.session_closed(id);
        let _ = std::fs::remove_file(self.scrollback_path(id));
        // Chat: Verlauf mit löschen (wie der Scrollback eines Terminals); eine laufende Antwort endet mit dem Entry.
        chatmod::store::remove(&self.dir, id);
        // Die geschlossene Id aus allen Agenten-Ketten nehmen (sie wird nie neu vergeben, #152).
        taint::prune(&mut self.entries);
        if self.layout.is_none() {
            // Letzter Bereich geschlossen: eine andere (nicht minimierte) Sitzung zeigen, sonst leerer Hub.
            if let Some(first) = self.unminimized_docked().first().copied() {
                self.activate(first);
            }
        }
    }

    /// Strg+Tab: reihum durch die Sitzungen des Fensters/Projekts, auch durch minimierte (#162: nachdem eine minimierte
    /// Sitzung allein geöffnet wurde, sind die übrigen minimiert – Strg+Tab erreicht sie trotzdem). Eine minimierte öffnet
    /// sich dabei wie per Klick allein (`activate`).
    fn cycle(&mut self, delta: i32) {
        let ids: Vec<u64> = self.docked_ids();
        let Some(next) = cycle_target(&ids, self.focused, delta) else { return };
        let Some(e) = self.entries.get_mut(&next) else { return };
        e.attention = false;
        self.detached.retain(|d| *d != next);
        // Entscheidung des Nutzers: im fokussierten Bereich zeigen (`replace`), die Teilung bleibt; der verdrängte Bereich
        // wird minimiert. Kein Hinweis (jeder Schritt verdrängt höchstens einen Bereich).
        place_session(&mut self.layout, &mut self.entries, next, self.focused, crate::layout::Placement::Replace, &|_| false);
        self.focused = next;
        self.want_focus = Some(next);
    }

    fn save_all(&self) {
        state::save_json(&self.dir.join("settings.json"), &self.settings);
        state::save_json(&self.dir.join("prompts.json"), &self.prompts);
        let _ = std::fs::create_dir_all(self.dir.join("scrollback"));
        self.save_state();
        for (id, e) in &self.entries {
            if let Some(s) = &e.live {
                let _ = state::write_atomic(&self.scrollback_path(*id), s.dump_text(3000).as_bytes());
            }
        }
        self.save_chats();
        self.ed.save(&self.dir);
    }

    /// Nur `state.json` schreiben (Sitzungen, Layout, Projekte) – leicht genug für sofortiges Speichern, z. B. wenn sich
    /// eine Agenten-Kette ändert (#152).
    /// Mit „Sitzungen merken“ aus (seit dem Start) steht dort nur der zugeschnittene Zustand – sonst würde ein Absturz beim
    /// nächsten Start die Wegwerf-Sitzungen sichern und dabei die einzige echte Sicherung ersetzen (#219).
    fn save_state(&self) {
        let throwaway = fresh_start::throwaway_run(self.settings.remember_sessions, self.remember_at_start);
        let s = fresh_start::state_for_disk(self.saved_state(), throwaway, state::persisted_lang(self.settings.language));
        state::save_json(&self.dir.join("state.json"), &s);
    }

    /// Inhalt von `state.json` (Sitzungen, Projekte mit dem aktuellen Layout, Fenster).
    fn saved_state(&self) -> SavedState {
        SavedState {
            sessions: self.entries.values().map(|e| e.meta.clone()).collect(),
            layout: self.layout.clone(),
            focused: self.focused,
            next_id: self.next_id,
            next_project_id: self.next_project_id.max(self.projects.iter().map(|p| p.id).max().unwrap_or(0) + 1),
            projects: {
                let mut p = self.projects.clone();
                if let Some(a) = p.iter_mut().find(|p| p.id == self.active_project) {
                    a.layout = self.layout.clone();
                    a.focused = self.focused;
                }
                p
            },
            active_project: self.active_project,
            detached: self.detached.clone(),
            composer_draft: self.composer_text.clone(),
            extra_windows: self.saved_windows(),
        }
    }

    /// Verläufe geänderter Chats schreiben.
    fn save_chats(&self) {
        for (id, e) in &self.entries {
            if let Some(c) = &e.chat {
                if c.dirty() {
                    if chatmod::store::save(&chatmod::store::path(&self.dir, *id), &c.file_for_save()).is_ok() {
                        c.mark_saved();
                    }
                }
            }
        }
    }

    /// Stil (Thema, Deckkraft, Animationen, Akzentfarbe) auf den Kontext anwenden. Der egui-Stil gilt für alle Fenster
    /// gemeinsam; jedes Fenster setzt vor dem Zeichnen seinen eigenen (Akzent = Projektfarbe seiner Todo-Ansicht, #110).
    /// Fertige Stile liegen im Zwischenspeicher, damit der Wechsel zwischen Fenstern `style::apply` nicht jedes Bild neu rechnet.
    fn ensure_style(&mut self, ctx: &egui::Context) {
        let theme = self.theme();
        // Deckkraft der Seitenleiste (mit eigenem Hintergrund des Themes die des Bereichs, #387).
        let side = self.surface_alpha(crate::theme::background::Area::Sidebar);
        let key = format!("{}|{}|{}|{:?}|{side}", theme.name, theme.bg.a(), self.settings.animations, style::accent());
        if self.applied_theme == key {
            return;
        }
        style::set_animations(self.settings.animations);
        // Oberflächenfarben (#309) gelten global; auch bei einem Treffer im Zwischenspeicher setzen.
        style::set_theme_ui(theme.ui_colors());
        match self.style_cache.iter().find(|(k, _)| *k == key) {
            Some((_, s)) => ctx.set_style(s.clone()),
            None => {
                style::apply(ctx, &theme, side);
                if self.style_cache.len() >= 16 {
                    self.style_cache.remove(0);
                }
                self.style_cache.push((key.clone(), ctx.style()));
                // Ein neuer Theme-Akzent erreicht `sync_accent` erst im nächsten Bild – dann den Stil noch einmal bauen.
                ctx.request_repaint();
            }
        }
        self.applied_theme = key;
    }

    /// Theme und Zellgröße (physische Pixel), mit denen eine Sitzung Farb- und Größenabfragen beantwortet (#183) – wie
    /// beim Zeichnen in `show_pane`: Theme des Profils, sonst das der App; Schriftgröße des Profils, sonst die der App.
    /// Vor dem ersten Frame (noch keine Schriften) ohne Zellgröße.
    fn reply_view_for(&self, ctx: &egui::Context, profile_id: u64) -> crate::terminal::ReplyView {
        let base = self.theme();
        let prof = self.settings.profiles.iter().find(|p| p.id == profile_id);
        let theme = match prof.filter(|p| !p.theme.is_empty()).and_then(|p| self.themes.iter().find(|t| t.name == p.theme)) {
            Some(t) => {
                let mut t = t.clone();
                t.bg = Color32::from_rgba_unmultiplied(t.bg.r(), t.bg.g(), t.bg.b(), base.bg.a());
                t
            }
            None => base,
        };
        let size = prof.filter(|p| p.font_size > 0.0).map(|p| p.font_size).unwrap_or(self.settings.font_size);
        let (cell_w, cell_h) = if ctx.cumulative_pass_nr() > 0 {
            let cs = ctx.fonts_mut(|f| f.layout_no_wrap("M".to_string(), FontId::monospace(size), Color32::WHITE).size());
            let ppp = ctx.pixels_per_point();
            ((cs.x * ppp).round().clamp(0.0, u16::MAX as f32) as u16, (cs.y * ppp).round().clamp(0.0, u16::MAX as f32) as u16)
        } else {
            (0, 0)
        };
        crate::terminal::ReplyView { theme, cell_w, cell_h }
    }

    /// Alle laufenden Sitzungen mit Theme und Zellgröße versorgen (jedes Frame): auch nicht sichtbare beantworten
    /// Farb- und Größenabfragen richtig, und ein Theme-Wechsel erreicht sie sofort.
    fn sync_reply_views(&self, ctx: &egui::Context) {
        let mut cache: Vec<(u64, crate::terminal::ReplyView)> = Vec::new();
        for e in self.entries.values() {
            let Some(s) = &e.live else { continue };
            let p = e.meta.profile;
            if !cache.iter().any(|(id, _)| *id == p) {
                cache.push((p, self.reply_view_for(ctx, p)));
            }
            if let Some((_, v)) = cache.iter().find(|(id, _)| *id == p) {
                s.seed_view(&v.theme, v.cell_w, v.cell_h);
            }
        }
    }

    fn theme(&self) -> Theme {
        let mut t = self
            .themes
            .iter()
            .find(|t| t.name == self.settings.theme)
            .unwrap_or(&self.themes[0])
            .clone();
        // „Transparenz reduzieren“ (#387): alles deckend.
        let o = if self.settings.reduce_transparency { 1.0 } else { self.settings.opacity.clamp(0.3, 1.0) };
        let a = (o * 255.0).round() as u8;
        t.bg = Color32::from_rgba_unmultiplied(t.bg.r(), t.bg.g(), t.bg.b(), a);
        t
    }

    // ---------------------------------------------------------------- UI

    /// Reiterwechsel per Tastatur (gilt in jedem Bereich; Terminals laufen im Hintergrund unverändert weiter).
    fn mode_shortcuts(&mut self, ctx: &egui::Context) {
        if self.capture_key.is_some() {
            return;
        }
        for (id, mode) in [
            ("mode_terminals", titlebar::Mode::Terminals),
            ("mode_tasks", titlebar::Mode::Tasks),
            ("mode_terminals_alt", titlebar::Mode::Terminals),
            ("mode_tasks_alt", titlebar::Mode::Tasks),
            ("mode_editor", titlebar::Mode::Editor),
            ("mode_editor_alt", titlebar::Mode::Editor),
            ("mode_time", titlebar::Mode::Time),
            ("mode_time_alt", titlebar::Mode::Time),
        ] {
            if let Some((m, k)) = self.binding(id) {
                if ctx.input_mut(|i| consume_key_exact(i, m, k, true)) {
                    self.set_mode(mode);
                }
            }
        }
    }

    /// Bereich wechseln; beim Zurück in die Terminals bekommt das zuletzt aktive Terminal wieder den Fokus
    /// (verzögert, siehe `focus_return_tick`).
    fn set_mode(&mut self, mode: titlebar::Mode) {
        if self.mode != mode {
            self.focus_return = (mode == titlebar::Mode::Terminals).then_some(0);
            // Editor (#54): beim Wechsel dorthin hat das Textfeld der aktiven Teilung die Tastatur.
            self.ed.want_focus |= mode == titlebar::Mode::Editor;
        }
        if mode != titlebar::Mode::Tasks {
            self.plan.forget_terminal_return(); // von Hand weggeschaltet: der Rückweg „Zurück zum Terminal Hub“ gilt nicht mehr
        }
        self.mode = mode;
    }

    /// Am Ende jedes Bilds: steht ein Rückweg in den Terminal Hub an (`focus_return`), nach zwei vollen Terminal-Bildern
    /// (damit Ebenen des alten Reiters nicht mehr als „sichtbar“ zählen) das Ziel fokussieren. Das Ziel ist das zuletzt
    /// fokussierte Terminal, falls es im Layout steht, sonst die Pane oben links. Ist dann ein Dialog, Fenster oder Popup
    /// offen, bleibt der Fokus dort (#238).
    fn focus_return_tick(&mut self, ctx: &egui::Context) {
        let Some(n) = self.focus_return else { return };
        if self.mode != titlebar::Mode::Terminals {
            self.focus_return = None;
            return;
        }
        if n < 2 {
            self.focus_return = Some(n + 1);
            ctx.request_repaint();
            return;
        }
        self.focus_return = None;
        if focus_held_elsewhere(ctx) {
            return;
        }
        if let Some(id) = self.layout.as_ref().and_then(|l| return_focus_target(l, self.focused)) {
            self.want_focus = Some(id);
            ctx.request_repaint();
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        if self.capture_key.is_some() {
            return; // gerade wird ein Kürzel aufgenommen
        }
        let mut binds: Vec<(&'static str, Modifiers, Key)> =
            keys::DEFAULTS.iter().filter(|(id, _)| !keys::is_global(id) && !keys::is_editor(id)).filter_map(|(id, _)| self.binding(id).map(|(m, k)| (*id, m, k))).collect();
        // Spezifischere Kürzel (mehr Modifikatoren) zuerst prüfen.
        binds.sort_by_key(|b| std::cmp::Reverse(b.1.ctrl as u8 + b.1.shift as u8 + b.1.alt as u8));
        let mut fired: Vec<&'static str> = Vec::new();
        let mut extra: Vec<Action> = Vec::new();
        let mut alt_n: Option<usize> = None;
        let cs = Modifiers::CTRL | Modifiers::SHIFT;
        // Die festen Kürzel nur verbrauchen, wenn es ein Ziel gibt – sonst gehören die Tasten dem Terminal (Alt+Zahl ist
        // z. B. `digit-argument` in bash, #254).
        let (n_projects, n_sessions) = (self.projects.len(), self.project_ids().len());
        let dirs: Vec<(Key, Nav)> = [(Key::ArrowLeft, Nav::Left), (Key::ArrowRight, Nav::Right), (Key::ArrowUp, Nav::Up), (Key::ArrowDown, Nav::Down)]
            .into_iter()
            .filter(|(_, n)| focus_target(&self.pane_rects, self.focused, *n).is_some())
            .collect();
        ctx.input_mut(|i| {
            for (id, m, k) in &binds {
                if consume_key_exact(i, *m, *k, false) {
                    fired.push(id);
                }
            }
            for (n, k) in [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9].into_iter().enumerate() {
                // Physisch: mit Umschalt liefert die Zifferntaste ein Zeichen („!“, „/“ …), nicht `Key::Num1`.
                if n < n_projects && consume_key_exact(i, cs, k, true) {
                    extra.push(Action::SwitchProject(n as u64 + 1_000_000));
                }
                if n < n_sessions && consume_key_exact(i, Modifiers::ALT, k, true) {
                    alt_n = Some(n);
                }
            }
            let ca = Modifiers::CTRL | Modifiers::ALT;
            for (k, n) in dirs {
                if consume_key_exact(i, ca, k, false) {
                    extra.push(Action::FocusDir(n));
                }
            }
        });
        let f = self.focused;
        for id in fired {
            actions.push(match id {
                "new" => Action::NewQuick,
                "split_r" => Action::Split(f, Dir::Horizontal),
                "split_d" => Action::Split(f, Dir::Vertical),
                "hide" => Action::MinimizePane(f),
                "rename" => Action::StartRename(f),
                "palette" => Action::OpenPalette,
                "search" => Action::OpenSearch,
                "broadcast" => Action::ToggleBroadcast,
                "notes" => Action::OpenNotes(f),
                "files" => Action::ToggleFiles,
                "editor" => Action::OpenEditor,
                "queue" => Action::OpenQueue(f),
                "composer" => Action::ToggleComposer,
                "diff" => Action::ToggleDiff,
                "search_all" => Action::OpenGlobalSearch,
                "prev_cmd" => Action::JumpCmd(-1),
                "next_cmd" => Action::JumpCmd(1),
                "next" => Action::Cycle(1),
                "close_all" => Action::CloseAll(closing::CloseScope::All),
                "reload" => Action::RequestReload(f),
                _ => Action::Cycle(-1),
            });
        }
        for a in extra {
            actions.push(match a {
                Action::SwitchProject(n) if n >= 1_000_000 => match self.projects.get((n - 1_000_000) as usize) {
                    Some(p) => Action::SwitchProject(p.id),
                    None => continue,
                },
                other => other,
            });
        }
        if let Some(n) = alt_n {
            if let Some(id) = self.project_ids().get(n).copied() {
                actions.push(Action::Activate(id));
            }
        }
    }

    fn central(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        // Dialog „Neuer Chat“ offen: Terminals und Chat-Eingaben dahinter bekommen keine Tasten (Enter gehört dem Dialog).
        // Ebenso die Rückfrage „Schließen“: kein Terminal hält den Fokus (der liegt auf „Abbrechen“), Esc/Enter gehören ihr.
        // Die Rückfrage vor dem Einfügen mehrerer Zeilen zeichnet erst nach den Terminals: ohne das bekäme das Terminal
        // im ersten Bild noch Tasten (#217).
        if self.new_chat.is_some() || self.confirm_close.is_some() || self.link_ask_here(ctx) || self.paste_ask_here(None) {
            models_ui::release_focus(ctx);
        }
        // Weitere Dialoge über dem Terminal (#255): Neue Sitzung, Prompt an alle, Vorlagen, Worktree schließen. Nur ein
        // Terminal gibt den Fokus ab – ein Feld des Dialogs behält ihn.
        // Ebenso der Editor der Vorlagen-Bibliothek und die Rückfrage vor dem Start einer Vorlage (#244).
        // Ebenso die Auswahl der Konversation (#302, nur im Hauptfenster): Enter/Esc gehören ihr, nicht dem Terminal.
        let pick_here = self.wins.cur == 0 && self.conv_pick.open();
        if self.new_dialog.is_some() || self.folder_prompt.as_ref().is_some_and(|p| p.kind == 2) || self.templates_open || self.wt_close.is_some() || self.tpl.form.is_some() || self.tpl.confirm.is_some() || pick_here {
            self.release_terminal_focus(ctx);
        }
        // Terminals mit der Deckkraft ihres Bereichs; mit eigenem Hintergrund (#387) lässt die Mitte das Bild durch.
        let theme = self.theme_for(crate::theme::background::Area::Terminals);
        let chat_bg = self.theme_for(crate::theme::background::Area::Chat).bg;
        let center_fill = if self.backdrop_active() { Color32::TRANSPARENT } else { theme.bg };
        let font = FontId::monospace(self.settings.font_size);
        let lang = self.settings.language;

        self.sync_reply_views(ctx);
        // Broadcast: nur die fokussierte Sitzung zeichnet Eingaben mit.
        for (id, e) in &self.entries {
            if let Some(s) = &e.live {
                s.set_tee(self.broadcast && *id == self.focused);
                s.set_shift_enter_newline(self.settings.shift_enter_newline_powershell);
            }
        }
        {
            let n = self.layout.as_ref().map(Node::leaf_count).unwrap_or(0);
            egui::TopBottomPanel::top(style::salted(Id::new("bcbar")))
                .frame(egui::Frame::NONE.fill(Color32::from_rgb(0x8a, 0x4b, 0x2f)).inner_margin(egui::Margin::symmetric(10, 4)))
                .show_animated(ctx, self.broadcast, |ui| {
                    ui.horizontal(|ui| {
                        ui.colored_label(
                            Color32::WHITE,
                            if n > 1 {
                                format!("{} {}", lang.t("Broadcast aktiv – Eingaben gehen an alle sichtbaren Bereiche:", "Broadcast on – input goes to all visible panes:"), n)
                            } else {
                                lang.t("Broadcast aktiv – teile den Bereich, damit es Ziele gibt.", "Broadcast on – split a pane to have targets.").to_string()
                            },
                        );
                        if ui.small_button(lang.t("Beenden", "Stop")).clicked() {
                            actions.push(Action::ToggleBroadcast);
                        }
                    });
                });
        }

        self.pane_rects.clear();
        let hide_key = self.binding_text("hide");
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(center_fill))
            .show(ctx, |ui| {
                let rect = ui.available_rect_before_wrap();
                // Ausgabe eines Prozesses der Prozessliste (#243) statt der Sitzungen.
                if self.proc_pane(ui, rect, &theme, &font) {
                    return;
                }
                let Some(mut layout) = self.layout.take() else {
                    let kind = self.empty_heading();
                    let hints = self.empty_hints();
                    empty_hub::empty_state(ui, rect, lang, self.settings.claude_mode, kind, &hints, actions);
                    return;
                };
                let mut cx = PaneCx {
                    entries: &mut self.entries,
                    theme: &theme,
                    font: &font,
                    focused: self.focused,
                    want_focus: &mut self.want_focus,
                    actions,
                    lang,
                    claude_mode: self.settings.claude_mode,
                    rects: &mut self.pane_rects,
                    broadcast: self.broadcast,
                    usage: &self.usage,
                    git: &self.git,
                    dragging: &mut self.dragging,
                    detached: false,
                    themes: &self.themes,
                    profiles: &self.settings.profiles,
                    tl: &self.tlinks,
                    alt_wheel: self.settings.alt_screen_wheel,
                    hide_key: &hide_key,
                    chat_bg,
                };
                show_node(ui, &mut layout, rect, &mut cx);
                self.layout = Some(layout);
            });

        // Broadcast weiterleiten
        if self.broadcast {
            let data = self.entries.get(&self.focused).and_then(|e| e.live.as_ref()).map(|s| s.take_tee()).unwrap_or_default();
            if !data.is_empty() {
                // Tasten je Ziel mit dessen Modus kodieren (kitty, win32-input-mode, Cursor-Tasten, Bracketed Paste).
                for id in self.broadcast_targets() {
                    if let Some(s) = self.entries.get(&id).and_then(|e| e.live.as_ref()) {
                        let bytes = crate::vtkeys::TeeInput::encode_all(&data, s.key_mode());
                        if !bytes.is_empty() {
                            s.write(&bytes);
                        }
                    }
                }
            }
        }
    }

    /// Ziele des Broadcasts: alle Bereiche im Layout außer dem fokussierten.
    pub(super) fn broadcast_targets(&self) -> Vec<u64> {
        let mut leaves = Vec::new();
        if let Some(l) = &self.layout {
            l.leaves(&mut leaves);
        }
        leaves.retain(|i| *i != self.focused);
        leaves
    }

    /// Ordner für Git-Abfrage und Transkript-Suche aktuell halten (alle 2 s).
    fn sync_watchers(&mut self, ctx: &egui::Context) {
        if self.last_sync.elapsed() < Duration::from_secs(2) {
            return;
        }
        self.last_sync = Instant::now();
        let projects = history::projects_dir();
        // Die Suche im Projektverzeichnis höchstens alle 15 s (sie läuft im UI-Thread), nicht bei jedem Durchgang.
        let scan_now = self.last_transcript_scan.elapsed() >= Duration::from_secs(15);
        if scan_now {
            self.last_transcript_scan = Instant::now();
        }
        let mut fallback_done: Vec<(u64, String)> = Vec::new();
        for (id, e) in self.entries.iter_mut() {
            if e.is_chat() {
                continue; // kein Ordner, kein Transkript
            }
            self.git.set_dir(*id, PathBuf::from(e.cwd_now()));
            // Transkript über die Konversations-ID finden, falls noch nicht bekannt.
            if e.meta.transcript.is_empty() && !e.meta.claude_session.is_empty() {
                if let Some(distro) = e.eff_shell().strip_prefix("wsl:") {
                    // WSL: Verzeichnis liegt in der Distribution; die Suche läuft im Hintergrund.
                    self.usage.set_locate(*id, distro, &e.meta.claude_session);
                } else if scan_now {
                    if let Some(root) = &projects {
                        if let Ok(rd) = std::fs::read_dir(root) {
                            for d in rd.flatten() {
                                let p = d.path().join(format!("{}.jsonl", e.meta.claude_session));
                                if p.is_file() {
                                    e.meta.transcript = p.to_string_lossy().to_string();
                                    self.usage.set_path(*id, p);
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            // Ohne Hook-Daten (z. B. WSL ohne funktionierenden Interop): einfachen Status aus dem Transkript ableiten.
            if e.is_claude() && !e.hook_seen && !e.exited() {
                // Nach dem Ende von Claude (Prompt der Shell) zählt das Transkript erst wieder, wenn es weitergeht.
                if let Some(u) = self.usage.get(*id).filter(|u| e.transcript_status_ok(u.modified)) {
                    // Großzügige Frist: Claude schreibt bei langen Werkzeugläufen (Tests, Builds) lange nichts ins Transkript;
                    // ein zu früh erkanntes „bereit“ würde den nächsten Warteschlangen-Prompt mitten in die Arbeit tippen.
                    let recent = u.modified.and_then(|m| m.elapsed().ok()).is_some_and(|d| d < Duration::from_secs(20));
                    let old = e.status;
                    e.status = if recent { ClaudeStatus::Working } else { ClaudeStatus::Idle };
                    if e.queue_tick(old, e.status) {
                        fallback_done.push((*id, e.display_name()));
                    }
                }
            }
        }
        self.queue_finished(ctx, fallback_done);
    }


    fn project_bar(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let lang = self.settings.language;
        let projects: Vec<(u64, String, [u8; 3], usize, bool)> = self
            .projects
            .iter()
            .map(|p| (p.id, p.name.clone(), p.color, self.entries.values().filter(|e| e.meta.project == p.id && self.in_win(e)).count(), p.agent_access.level == crate::access::AreaLevel::Off))
            .collect();
        let bar = ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(4.0, 4.0);
            for (id, name, c, n, locked) in projects {
                let col = Color32::from_rgb(c[0], c[1], c[2]);
                // Schloss: für Agenten gesperrt (#386).
                let r = style::pill_ex(ui, &name, Some(n), col, id == self.active_project, locked);
                let r = if locked { r.on_hover_text(lang.t("Für Agenten gesperrt (Rechtsklick → Zugriff für Agenten …)", "Locked for agents (right-click → Agent access …)")) } else { r };
                if r.clicked() {
                    actions.push(Action::SwitchProject(id));
                }
                r.context_menu(|ui| {
                    if ui.button(lang.t("Umbenennen …", "Rename …")).clicked() {
                        actions.push(Action::ProjectPrompt(id, PromptKind::RenameProject));
                        ui.close();
                    }
                    if ui.button(lang.t("Standardordner …", "Default folder …")).clicked() {
                        actions.push(Action::ProjectPrompt(id, PromptKind::ProjectFolder));
                        ui.close();
                    }
                    if ui.button(lang.t("Zugriff für Agenten …", "Agent access …")).clicked() {
                        self.mcp.access_dlg = Some(crate::access::Area::Project(id));
                        ui.close();
                    }
                    if !self.settings.profiles.is_empty() {
                        ui.menu_button(lang.t("Standardprofil", "Default profile"), |ui| {
                            if ui.button(lang.t("Keins", "None")).clicked() {
                                actions.push(Action::SetProjectProfile(id, 0));
                                ui.close();
                            }
                            for p in &self.settings.profiles {
                                if ui.button(&p.name).clicked() {
                                    actions.push(Action::SetProjectProfile(id, p.id));
                                    ui.close();
                                }
                            }
                        });
                    }
                    palette_ui::color_menu(ui, lang.t("Farbe", "Color"), |ui| {
                        let cur = crate::palette::hex_of(c);
                        if let Some(h) = palette_ui::pick(ui, ("project_color", id), crate::palette::Area::Hub, Some(&cur), palette_ui::PickOpts::HUB, lang) {
                            actions.push(Action::SetProjectColor(id, crate::palette::rgb(&h)));
                            ui.close();
                        }
                    });
                    if n > 0 {
                        ui.separator();
                        if ui.button(lang.t("Alle Sitzungen des Projekts schließen …", "Close all sessions of the project …")).clicked() {
                            actions.push(Action::CloseAll(closing::CloseScope::Project(id)));
                            ui.close();
                        }
                    }
                    if self.projects.len() > 1 {
                        ui.separator();
                        // Rückfrage mit Wartezeit statt „zweimal klicken“ (#276, `closing`).
                        if ui.button(lang.t("Projekt löschen (Sitzungen bleiben) …", "Delete project (sessions are kept) …")).clicked() {
                            actions.push(Action::DeleteProjectAsk(id));
                            ui.close();
                        }
                    }
                });
            }
            if style::icon_btn_small(ui, SIcon::Plus, lang.t("Neues Projekt", "New project")).clicked() {
                actions.push(Action::NewProjectPrompt);
            }
        });
        bar.response.spot(Spot::ProjectBar);
    }

    /// Git-Befehl im Hintergrund ausführen; Ergebnis als Hinweis anzeigen und Ansicht aktualisieren.
    fn git_bg(&self, ctx: &egui::Context, ok_msg: &str, f: impl FnOnce() -> Result<String, String> + Send + 'static) {
        let (tx, ok_msg, diff, ctx) = (self.bg_tx.clone(), ok_msg.to_string(), self.diff.clone(), ctx.clone());
        std::thread::spawn(move || {
            let msg = match f() {
                Ok(_) => ok_msg,
                Err(e) => format!("Git: {e}"),
            };
            let _ = tx.send(msg);
            diff.refresh();
            ctx.request_repaint();
        });
    }

    fn poll_background(&mut self, ctx: &egui::Context) {
        while let Ok(m) = self.bg_rx.try_recv() {
            if !m.is_empty() {
                self.notify_text(m);
            }
        }
        if self.commit_ok.swap(false, std::sync::atomic::Ordering::SeqCst) {
            self.commit_msg.clear();
        }
        // Worktree fertig angelegt? (Ein abgebrochener Anlage-Thread darf die Anlage nicht ewig „hängen“ lassen.)
        let ready = self.pending_wt.as_ref().and_then(|p| match p.rx.try_recv() {
            Ok(r) => Some(r),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Err(self.settings.language.t("abgebrochen", "aborted").to_string())),
        });
        if let Some(res) = ready {
            let Some(p) = self.pending_wt.take() else { return };
            match res {
                Ok(()) => {
                    let mut opts = p.opts;
                    // Inzwischen ein anderes Projekt gewählt: die Sitzung still in ihrem Projekt anlegen und Bescheid geben,
                    // statt den Nutzer zurückzuholen oder sie ins falsche Layout zu legen (#258).
                    let elsewhere = opts.window.is_none_or(|w| w == self.wins.cur) && opts.project.is_some_and(|pr| pr != self.active_project);
                    opts.quiet |= elsewhere;
                    let id = self.new_session(ctx, opts);
                    if let Some(e) = self.entries.get_mut(&id) {
                        e.meta.worktree = Some(p.wt);
                    }
                    if elsewhere {
                        let lang = self.settings.language;
                        let name = self.entries.get(&id).map(|e| e.display_name()).unwrap_or_default();
                        let proj = self.entries.get(&id).and_then(|e| self.projects.iter().find(|p| p.id == e.meta.project)).map(|p| p.name.clone()).unwrap_or_default();
                        self.notify_text(match lang {
                            Lang::De => format!("Worktree-Sitzung {} ist im Projekt {} bereit.", lang.q(&name), lang.q(&proj)),
                            Lang::En => format!("Worktree session {} is ready in project {}.", lang.q(&name), lang.q(&proj)),
                        });
                    }
                }
                Err(e) => self.notify_text(format!("{}: {e}", self.settings.language.t("Worktree konnte nicht angelegt werden", "Could not create worktree"))),
            }
        }
    }

    /// Arbeitsordner eines Chats als Windows-Pfad (#392): Ein Linux-Pfad (`/home/u/p`, `~`) gilt in der Pfadart der Shell
    /// des Projekts (WSL-Distribution, Git Bash), sonst in der WSL-Standard-Distribution; geht das nicht, bleibt er.
    pub(super) fn chat_root(&self, cwd: &str, project: u64) -> String {
        let c = cwd.trim();
        if !crate::wsl::looks_linux(c) || !cfg!(windows) {
            return cwd.to_string();
        }
        let shell = self.shell_by_id(&self.effective_shell_id(None, project));
        cli_path_kind(shell.as_ref(), c).to_windows(c).unwrap_or_else(|_| cwd.to_string())
    }

    /// Pfad aus `--open`/`--edit` (#392): Ein Linux-Pfad (`/…`, `~`) gilt in der Sitzung `origin`, aus der der Aufruf kam
    /// (ihre Pfadart: WSL-Distribution, Git Bash), sonst in der WSL-Standard-Distribution; Ergebnis ein Windows-Pfad.
    /// Andere Pfade bleiben.
    pub(super) fn cli_path(&self, path: &str, origin: Option<u64>) -> Result<String, crate::targetpath::PathErr> {
        if !crate::wsl::looks_linux(path.trim()) {
            return Ok(path.to_string());
        }
        cli_path_kind(origin.and_then(|id| self.entries.get(&id)).and_then(|e| e.live.as_ref().map(|s| &s.spec)), path).to_windows(path)
    }

    /// Hinweis: Pfad nicht eingefügt (#392: Ordner einer anderen WSL-Distribution, nicht umrechenbar).
    pub(super) fn path_refused_toast(&mut self, err: &crate::targetpath::PathErr) {
        let lang = self.settings.language;
        let text = format!("{} {}", lang.t("Pfad nicht eingefügt:", "Path not inserted:"), err.message(lang));
        self.push_toast(text, toasts::ToastKind::Warning, None);
    }

    fn notify_text(&mut self, text: String) {
        self.notice = Some((text, Instant::now()));
    }

    /// Ordner (OSC) und beendete Befehle der Shell-Integration auswerten.
    fn poll_shell(&mut self, ctx: &egui::Context) {
        // Angesehen ist die fokussierte Sitzung des Fensters im Vordergrund (auch eines weiteren Fensters).
        let (window_focused, focused_id) = self.seen_focus();
        let threshold = self.settings.cmd_notify_secs as u64 * 1000;
        let lang = self.settings.language;
        let mut toasts: Vec<(u64, String, CmdDone)> = Vec::new();
        let mut copies: Vec<(u64, crate::terminal::ClipboardRequest)> = Vec::new();
        let paste_warn = self.settings.warn_multiline_paste;
        for (id, e) in self.entries.iter_mut() {
            let Some(s) = e.live.as_ref() else { continue };
            // Fokusmeldungen (DECSET 1004): gilt als fokussiert wie bei OSC 52 – fokussierte Sitzung des Fensters im
            // Vordergrund; abgedockte Sitzungen meldet ihr eigenes Fenster (`detached_windows`). Gesendet wird nur beim Wechsel.
            if !self.detached.contains(id) {
                s.report_focus(window_focused && *id == focused_id);
            }
            s.set_paste_warn(paste_warn);
            // Immer abholen, auch bei abgeschalteter Einstellung: ein alter Wunsch darf nicht später doch noch greifen.
            if let Some(req) = s.take_clipboard() {
                copies.push((*id, req));
            }
            let (cwd, done) = {
                let mut st = s.shell_state.lock().unwrap_or_else(|e| e.into_inner());
                (st.cwd.take(), std::mem::take(&mut st.done))
            };
            if let Some(c) = cwd {
                e.live_cwd = pathfmt::from_wsl(e.eff_shell(), &c);
                if e.start_cwd.is_empty() {
                    e.start_cwd = e.live_cwd.clone();
                }
            }
            for d in done {
                if threshold > 0 && d.dur_ms.unwrap_or(0) >= threshold && (!window_focused || *id != focused_id) {
                    e.attention = true;
                    toasts.push((*id, e.display_name(), d));
                }
            }
        }
        // Auch ohne neue Wünsche: wartende Texte nachreichen (Ratenbegrenzung).
        self.osc52_poll(ctx, copies);
        for (sid, name, d) in toasts {
            let secs = d.dur_ms.unwrap_or(0) / 1000;
            let dur = if secs >= 60 { format!("{} min {} s", secs / 60, secs % 60) } else { format!("{secs} s") };
            let ok = d.exit.unwrap_or(0) == 0;
            let body = format!(
                "{} · {} {}",
                if ok { lang.t("Befehl beendet", "Command finished") } else { lang.t("Befehl fehlgeschlagen", "Command failed") },
                lang.t("Dauer", "took"),
                dur
            );
            if !window_focused {
                if self.settings.notify_toast {
                    notify::toast(&name, &body, self.settings.notify_sound);
                }
                if self.settings.notify_flash {
                    ctx.send_viewport_cmd_to(self.attention_viewport(sid), egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
                }
            } else {
                self.notify_text(format!("{name}: {body}"));
            }
        }
    }

    /// Laufende Antworten aller Chats abholen (Hauptdurchgang, für alle Fenster). Fertige Antworten werden sofort
    /// gespeichert; in einer nicht angesehenen Sitzung gibt es eine Markierung.
    ///
    /// Danach die Werkzeug-Schleife jedes Chats (`chat::driver::drive` über [`ChatHost`]): Werkzeuge laufen hier im
    /// Hub mit dem Chat als Aufrufer. Wartet ein schreibender Aufruf auf Freigabe (oder die einmalige Frage nach der
    /// Stufe), steht der Chat auf „Freigabe?“ (wie eine Claude-Abfrage: Markierung in der Seitenleiste; MCP-Agenten
    /// schreiben dann nicht in den Chat).
    fn poll_chats(&mut self, ctx: &egui::Context) {
        let lang = self.settings.language;
        if self.llm_conn.poll(&mut self.settings.llm_servers, lang) {
            state::save_json(&self.dir.join("settings.json"), &self.settings);
        }
        let (window_focused, focused_id) = self.seen_focus();
        let ids: Vec<u64> = self.entries.iter().filter(|(_, e)| e.chat.is_some()).map(|(id, _)| *id).collect();
        let mut tools_running = false;
        for id in ids {
            let (finished, was_waiting) = {
                let Some(e) = self.entries.get_mut(&id) else { continue };
                let waiting = e.status == ClaudeStatus::Permission;
                let Some(c) = e.chat.as_mut() else { continue };
                if let Some(m) = &e.meta.chat {
                    c.notice = chatmod::llm_backend::chat_notice(&self.settings.llm_servers, m, lang);
                }
                (c.poll().finished, waiting)
            };
            // Stufe inzwischen im Reiter „Modelle“ gewählt: die Frage erübrigt sich (die Nachricht bleibt im Feld).
            if !self.chat_level_unchosen(id) {
                if let Some(c) = self.entries.get_mut(&id).and_then(|e| e.chat.as_mut()).filter(|c| c.level_question.is_some()) {
                    c.level_question = None;
                    c.touch();
                }
            }
            let driven = chatmod::driver::drive(&mut ChatHost { app: self, ctx }, id);
            if driven.more {
                ctx.request_repaint();
            }
            let Some(e) = self.entries.get_mut(&id) else { continue };
            let Some(c) = e.chat.as_ref() else { continue };
            // Offene Fragen an den Nutzer (#356) haben kein Zeitlimit: dafür nicht ständig neu zeichnen.
            tools_running |= c.tool_running() && !c.awaiting_answer();
            e.status = chat_claude_status(c.status());
            let waiting = e.status == ClaudeStatus::Permission;
            // Markierung: Runde zu Ende (fertig, Fehler, angehalten) oder eine Freigabe-Frage erscheint.
            let done = finished && !c.busy();
            let unseen = !window_focused || id != focused_id;
            if (done || (waiting && !was_waiting) || (driven.changed && c.paused)) && unseen {
                e.attention = true;
            }
            if (waiting && !was_waiting) && unseen {
                ctx.send_viewport_cmd_to(self.attention_viewport(id), egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
            }
            if finished || driven.changed {
                self.save_chat(id);
            }
        }
        if tools_running {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        self.poll_chat_images(ctx);
        self.refresh_chat_context();
    }

    /// Füllstand des Kontexts (#339) für Chats ohne laufende Runde schätzen: so, wie die nächste Anfrage aussähe – nur,
    /// wenn sich Verlauf, Einstellungen der Modelle, Expertenmodus, Abgleich oder Projekt/Ordner geändert haben.
    /// Während einer Runde kommt die Aufteilung mit der Anfrage (`TurnEvent::Context`).
    fn refresh_chat_context(&mut self) {
        use std::hash::{Hash, Hasher};
        let idle: Vec<u64> = self.entries.iter().filter(|(_, e)| e.chat.as_ref().is_some_and(|c| !c.busy())).map(|(id, _)| *id).collect();
        if idle.is_empty() {
            return;
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
        serde_json::to_vec(&self.settings.llm_servers).unwrap_or_default().hash(&mut h);
        serde_json::to_vec(&self.settings.chat_prompt_parts).unwrap_or_default().hash(&mut h);
        crate::mcp::expert_mode().hash(&mut h);
        let base = h.finish();
        for id in idle {
            let Some(e) = self.entries.get(&id) else { continue };
            let (Some(c), Some(meta)) = (e.chat.as_ref(), e.meta.chat.as_ref()) else { continue };
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (base, c.rev(), c.token_scale.to_bits(), &meta.server, &meta.model, &e.meta.cwd).hash(&mut h);
            self.projects.iter().find(|p| p.id == e.meta.project).map(|p| &p.name).hash(&mut h);
            let key = h.finish();
            if c.context_key == Some(key) {
                continue;
            }
            let parts = chatmod::llm_backend::context_estimate(meta, c.messages(), &self.turn_context(id));
            if let Some(c) = self.entries.get_mut(&id).and_then(|e| e.chat.as_mut()) {
                c.context = parts;
                c.context_key = Some(key);
            }
        }
    }

    /// Was für die Werkzeuge eines Chats gerade gilt: Einstellungen seines Modells (unbekannter Server: Stufe „Aus“),
    /// Werkzeug-Fähigkeit und der Expertenmodus der Todo-Liste.
    fn chat_policy(&self, id: u64) -> chatmod::tools::ToolPolicy {
        let meta = self.entries.get(&id).and_then(|e| e.meta.chat.clone()).unwrap_or_default();
        // Vorfahren des Chats: seine gespeicherte Agenten-Kette und die Kette der laufenden Runde (#152).
        let origins = taint::ancestors(&self.entries, Some(id));
        match self.settings.llm_servers.iter().find(|s| s.id == meta.server) {
            Some(s) => chatmod::tools::ToolPolicy::new(s.model_cfg(&meta.model), s.tools_supported(&meta.model), self.plan.expert),
            None => chatmod::tools::ToolPolicy::new(crate::llm::config::LlmModel { level: crate::mcp::McpLevel::Off, ..crate::llm::config::LlmModel::named(&meta.model) }, None, self.plan.expert),
        }
        .with_origins(origins)
        .with_agent(self.agent_env(id))
        .with_project_cap(self.chat_project_cap(id))
    }

    /// Neue Kennung für ein Seitenleisten-Projekt (#386): steigt immer, auch über gelöschte Projekte hinweg.
    pub(super) fn alloc_project_id(&mut self) -> u64 {
        let id = self.next_project_id.max(self.projects.iter().map(|p| p.id).max().unwrap_or(0) + 1);
        self.next_project_id = id + 1;
        id
    }

    /// Stufe des Projekts eines Chats für seine Paneldeck-Werkzeuge (#386): „wie global“ = wie das Modell; gesperrt = keine.
    fn chat_project_cap(&self, id: u64) -> crate::mcp::McpLevel {
        let Some(project) = self.entries.get(&id).map(|e| e.meta.project) else { return crate::mcp::McpLevel::Off };
        // Obergrenze hier „Steuern“: die Stufe des Modells begrenzt `ToolPolicy` selbst (es gilt das Minimum).
        self.access_map(crate::mcp::McpLevel::Control).sidebar(crate::access::Caller::Home(project), project)
    }

    /// Muss der Chat vor dem Senden einmal nach der Stufe seines Modells fragen? Ja, wenn das Modell Werkzeuge nutzen
    /// könnte (Fähigkeit nicht „nein“) und der Nutzer die Stufe nie gewählt hat. Für P5 (`send_chat_message`): ist das
    /// wahr, kann ein externer Agent nicht senden (`permission_pending`) – die Frage beantwortet nur der Nutzer.
    pub(super) fn chat_level_unchosen(&self, id: u64) -> bool {
        let Some(meta) = self.entries.get(&id).and_then(|e| e.meta.chat.as_ref()) else { return false };
        let Some(s) = self.settings.llm_servers.iter().find(|s| s.id == meta.server) else { return false };
        s.tools_supported(&meta.model) != Some(false) && !s.model_cfg(&meta.model).level_chosen
    }

    /// Antwort auf die einmalige Frage: Stufe speichern (als gewählt) und – hatte der Nutzer die Frage mit „Senden“
    /// ausgelöst – seine zurückgehaltene Nachricht senden. Kam die Frage von einem Agenten (`permission_pending`), schließt
    /// die Antwort sie nur: Der Entwurf im Eingabefeld gehört dem Nutzer und geht nicht ungefragt los.
    fn chat_choose_level(&mut self, ctx: &egui::Context, id: u64, level: crate::mcp::McpLevel) {
        let Some(meta) = self.entries.get(&id).and_then(|e| e.meta.chat.clone()) else { return };
        if let Some(s) = self.settings.llm_servers.iter_mut().find(|s| s.id == meta.server) {
            let mut cfg = s.model_cfg(&meta.model);
            cfg.level = level;
            cfg.level_chosen = true;
            s.set_model_cfg(cfg);
            state::save_json(&self.dir.join("settings.json"), &self.settings);
        }
        let asker = self.entries.get_mut(&id).and_then(|e| e.chat.as_mut()).and_then(|c| {
            c.touch();
            c.level_question.take()
        });
        if send_after_level_choice(asker) {
            self.chat_action(ctx, id, &Action::ChatSend(id));
        }
    }

    fn save_chat(&self, id: u64) {
        if let Some(c) = self.entries.get(&id).and_then(|e| e.chat.as_ref()) {
            if chatmod::store::save(&chatmod::store::path(&self.dir, id), &c.file_for_save()).is_ok() {
                c.mark_saved();
            }
        }
    }

    /// Bedienung eines Chats (Senden, Stopp, Neu erzeugen, Bearbeiten) mit dem Backend des Hubs.
    /// Was das Backend für eine Antwort dieses Chats wissen muss (Projekt, Ordner, Server, Prompt-Einstellungen).
    fn turn_context(&self, id: u64) -> chatmod::TurnContext {
        let e = self.entries.get(&id);
        let project = e.and_then(|e| self.projects.iter().find(|p| p.id == e.meta.project)).map(|p| p.name.clone()).unwrap_or_default();
        chatmod::TurnContext {
            project,
            folder: e.map(|e| e.meta.cwd.clone()).unwrap_or_default(),
            servers: self.settings.llm_servers.list.clone(),
            lang: self.settings.language,
            expert: crate::mcp::expert_mode(),
            token_scale: e.and_then(|e| e.chat.as_ref()).map_or(1.0, |c| c.token_scale),
            prompt_parts: self.settings.chat_prompt_parts,
            agent: self.agent_env(id),
            // Für die Schätzung des Füllstands: so, wie die nächste Nachricht ginge (Schalter *Plan*, #358). Die Anfrage
            // selbst setzt `ChatState::start_turn`.
            plan: e.and_then(|e| e.chat.as_ref()).and_then(|c| c.next_plan_phase()),
            compact: self.settings.chat_compact,
            images: self.chat_turn_images(id),
        }
    }

    /// Eine Nachricht an einen Chat senden – die eine Stelle für das Eingabefeld (`text = None`: der Entwurf) und für
    /// externe Agenten (`text = Some(…)`, Entwurf bleibt unberührt; P5 `send_chat_message`).
    ///
    /// Wurde die Stufe des Modells nie gewählt, wird nichts gesendet: die einmalige Frage erscheint im Chat
    /// (`ChatState::level_question`) und das Ergebnis ist [`ChatSendResult::LevelQuestion`] – für Agenten
    /// `permission_pending`, nur der Nutzer kann sie beantworten.
    pub(super) fn chat_send(&mut self, ctx: &egui::Context, id: u64, text: Option<&str>, origin: ChatSendOrigin) -> ChatSendResult {
        let Some(c) = self.entries.get(&id).and_then(|e| e.chat.as_ref()) else { return ChatSendResult::NotAChat };
        let empty = match text {
            Some(t) => t.trim().is_empty(),
            None => c.draft.trim().is_empty(),
        };
        // `/compact` (#359) kompaktiert statt zu senden: keine neue Nachricht, an die eine Markierung gehörte.
        let compact_cmd = chatmod::compact::is_command(text.unwrap_or(&c.draft));
        if empty {
            return ChatSendResult::Empty;
        }
        if c.busy() {
            return ChatSendResult::Busy;
        }
        if !compact_cmd && self.chat_level_unchosen(id) {
            if let Some(e) = self.entries.get_mut(&id) {
                if let Some(c) = e.chat.as_mut() {
                    c.level_question = Some(level_asker(c.level_question, &origin));
                    c.touch();
                }
                e.status = ClaudeStatus::Permission;
            }
            return ChatSendResult::LevelQuestion;
        }
        // Kette der auslösenden Agenten: der Aufrufer und seine Vorfahren (gespeicherte Agenten-Kette, bei einem Chat
        // auch die Kette seiner laufenden Runde; transitiv, #152).
        let chain = match &origin {
            ChatSendOrigin::User => Vec::new(),
            ChatSendOrigin::Agent { caller, .. } => taint::handed_on(&self.entries, *caller),
        };
        let backend = self.chat_backend.clone();
        let rctx = ctx.clone();
        let repaint: chatmod::Repaint = std::sync::Arc::new(move || rctx.request_repaint());
        let tcx = self.turn_context(id);
        let Some(e) = self.entries.get_mut(&id) else { return ChatSendResult::NotAChat };
        let meta = e.meta.chat.clone().unwrap_or_default();
        let Some(c) = e.chat.as_mut() else { return ChatSendResult::NotAChat };
        let sent = match text {
            Some(t) => c.send_text(t, backend.as_ref(), &meta, &tcx, repaint),
            None => c.send(backend.as_ref(), &meta, &tcx, repaint),
        };
        if sent && !compact_cmd {
            // Herkunft der Runde: der Nutzer hebt eine Agenten-Herkunft auf; ein Agent markiert seine Nachricht.
            c.origin_sessions = chain;
            match origin {
                ChatSendOrigin::User => {}
                ChatSendOrigin::Agent { note, .. } => {
                    if let Some(i) = chatmod::logic::last_user(&c.file.messages) {
                        c.file.messages[i].note = note;
                        c.touch();
                    }
                }
            }
        }
        e.status = chat_claude_status(c.status());
        if !sent {
            return ChatSendResult::Busy;
        }
        self.save_chat(id);
        ChatSendResult::Sent
    }

    /// Nächste Anfrage einer Werkzeug-Runde (für `ChatHost::request`).
    fn chat_next_request(&mut self, ctx: &egui::Context, id: u64) {
        let backend = self.chat_backend.clone();
        let rctx = ctx.clone();
        let repaint: chatmod::Repaint = std::sync::Arc::new(move || rctx.request_repaint());
        let tcx = self.turn_context(id);
        let Some(e) = self.entries.get_mut(&id) else { return };
        let meta = e.meta.chat.clone().unwrap_or_default();
        if let Some(c) = e.chat.as_mut() {
            c.next_request(backend.as_ref(), &meta, &tcx, repaint);
        }
    }

    fn chat_action(&mut self, ctx: &egui::Context, id: u64, a: &Action) {
        // Senden: über `chat_send` (fragt bei nie gewählter Stufe einmal nach; die Nachricht wartet im Feld).
        if matches!(a, Action::ChatSend(_)) {
            self.chat_send(ctx, id, None, ChatSendOrigin::User);
            return;
        }
        if let Action::ChatLevel(_, level) = a {
            return self.chat_choose_level(ctx, id, *level);
        }
        let backend = self.chat_backend.clone();
        let rctx = ctx.clone();
        let repaint: chatmod::Repaint = std::sync::Arc::new(move || rctx.request_repaint());
        let tcx = if matches!(a, Action::ChatSend(_) | Action::ChatRegenerate(_) | Action::ChatCompact(_)) { self.turn_context(id) } else { chatmod::TurnContext::default() };
        let Some(e) = self.entries.get_mut(&id) else { return };
        let meta = e.meta.chat.clone().unwrap_or_default();
        let Some(c) = e.chat.as_mut() else { return };
        let changed = match a {
            Action::ChatSend(_) => c.send(backend.as_ref(), &meta, &tcx, repaint),
            Action::ChatRegenerate(_) => c.regenerate(backend.as_ref(), &meta, &tcx, repaint),
            Action::ChatStop(_) => {
                c.stop();
                c.level_question = None;
                true
            }
            Action::ChatEdit(_) => {
                c.begin_edit();
                false
            }
            // Ausgeführt wird erst im Hauptdurchgang (`poll_chats`), auch wenn der Chat in einem weiteren Fenster liegt.
            // Gilt nur, wenn genau dieser Aufruf noch wartet (veraltete Klicks bleiben ohne Wirkung).
            Action::ChatDecide(_, call_id, d) => {
                let ok = c.decide(call_id, *d);
                // Fokus zurück ins Eingabefeld: sonst gälte Enter auf dem Knopf an derselben Stelle gleich der nächsten Frage.
                c.focus_input = true;
                ok
            }
            Action::ChatContinue(_) => c.resume(),
            Action::ChatRevoke(_, rule) => c.revoke(rule),
            // Fehler zeigt der Chat selbst (`compact.failure`).
            Action::ChatCompact(_) => c.compact_now(backend.as_ref(), &meta, &tcx, repaint).is_ok(),
            _ => false,
        };
        e.status = chat_claude_status(c.status());
        if changed {
            self.save_chat(id);
        }
    }

    /// Gesamten Verlauf einer Sitzung als Textdatei speichern (Chat: als Markdown).
    fn export_history(&mut self, id: u64) {
        let lang = self.settings.language;
        let Some(e) = self.entries.get(&id) else { return };
        let (text, ext) = match (&e.chat, e.live.as_ref()) {
            (Some(c), _) => (chatmod::logic::to_markdown(&e.display_name(), e.chat_model(), c.messages(), lang), "md"),
            (None, Some(s)) => (s.dump_text(2_000_000), "txt"),
            (None, None) => return,
        };
        let safe: String = e.display_name().chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let name = format!("{safe}-{secs}.{ext}");
        let path = notify::save_file(&name).unwrap_or_else(|| {
            let d = self.dir.join("exports");
            let _ = std::fs::create_dir_all(&d);
            d.join(&name)
        });
        match std::fs::write(&path, text) {
            Ok(()) => self.notify_text(format!("{} {}", lang.t("Verlauf gespeichert:", "History saved:"), path.display())),
            Err(err) => self.notify_text(format!("{}: {err}", lang.t("Speichern fehlgeschlagen", "Saving failed"))),
        }
    }

    /// Hinweis, wenn das Tagesbudget überschritten ist (höchstens einmal pro Tag).
    fn check_budget(&mut self, ctx: &egui::Context) {
        if (self.settings.daily_budget <= 0.0 && self.settings.session_budget <= 0.0) || !self.settings.claude_mode || self.last_budget_check.elapsed() < Duration::from_secs(20) {
            return;
        }
        self.last_budget_check = Instant::now();
        // Budget je Sitzung (einmal je Sitzung)
        if self.settings.session_budget > 0.0 {
            let lang = self.settings.language;
            let limit = self.settings.session_budget;
            let mut over: Vec<(String, f64)> = Vec::new();
            for (id, e) in self.entries.iter_mut() {
                if e.budget_warned {
                    continue;
                }
                if let Some(u) = self.usage.get(*id) {
                    let c = u.cost();
                    if c > limit {
                        e.budget_warned = true;
                        over.push((e.display_name(), c));
                    }
                }
            }
            for (name, c) in over {
                let msg = format!("{} {} ≈ ${c:.2} > ${limit:.2}", lang.t("Sitzungsbudget überschritten:", "Session budget exceeded:"), lang.q(&name));
                self.notify_text(msg.clone());
                if !ctx.input(|i| i.viewport().focused.unwrap_or(true)) && self.settings.notify_toast {
                    notify::toast("Paneldeck", &msg, self.settings.notify_sound);
                }
            }
        }
        if self.settings.daily_budget <= 0.0 {
            return;
        }
        let today = usage::utc_date(usage::today_secs());
        if self.budget_warned_day == today {
            return;
        }
        let mut c = 0.0;
        for id in self.entries.keys() {
            if let Some(u) = self.usage.get(*id) {
                c += u.cost_by_day().get(&today).copied().unwrap_or(0.0);
            }
        }
        if c > self.settings.daily_budget {
            self.budget_warned_day = today;
            let lang = self.settings.language;
            let msg = format!("{} ≈ ${:.2} > ${:.2}", lang.t("Tagesbudget überschritten:", "Daily budget exceeded:"), c, self.settings.daily_budget);
            self.notify_text(msg.clone());
            if !ctx.input(|i| i.viewport().focused.unwrap_or(true)) && self.settings.notify_toast {
                notify::toast("Paneldeck", &msg, self.settings.notify_sound);
            }
        }
    }

    /// Ergebnis der Verlaufssuche des gerade gezeichneten Fensters abholen (jedes Fenster hat seine eigene Suche).
    fn poll_history_hits(&mut self) {
        if let Some(rx) = &self.history_hits_rx {
            if let Ok(h) = rx.try_recv() {
                self.history_hits = Some(h);
                self.history_hits_rx = None;
            }
        }
    }

    fn poll_history(&mut self) {
        self.poll_history_hits();
        if let Some(rx) = &self.history_rx {
            if let Ok(items) = rx.try_recv() {
                self.history = items;
                self.history_rx = None;
            }
        }
    }

    fn poll_hooks(&mut self, ctx: &egui::Context) {
        let (window_focused, seen_id) = self.seen_focus();
        // Ereignisse in der angesehenen Sitzung gelten als gesehen.
        if window_focused {
            if let Some(e) = self.entries.get_mut(&seen_id) {
                e.attention = false;
            }
        }
        let msgs: Vec<HookMsg> = self.hook_server.as_ref().map(|s| s.rx.try_iter().collect()).unwrap_or_default();
        let lang = self.settings.language;
        let mut queue_done: Vec<(u64, String)> = Vec::new();
        let mut queue_paused: Vec<u64> = Vec::new();
        for m in msgs {
            let focused_id = seen_id;
            let Some(e) = self.entries.get_mut(&m.session) else { continue };
            // Ein Chat hat keine Shell und keinen Claude: sein Status kommt nur aus `poll_chats`. Eine Hook-Meldung mit
            // seiner Kennung (gefälscht oder veraltet) darf ihn nicht ändern.
            if e.is_chat() {
                continue;
            }
            // Annahme (Schlüssel des laufenden Starts und Konversation, #260) und neuer Status: `Entry::take_hook`.
            let Some(claude_status::HookEffect { old, new, conv, keyless, advance, paused }) = e.take_hook(&m) else { continue };
            if paused {
                queue_paused.push(m.session);
            }
            let sh = e.eff_shell().to_string();
            let m = HookMsg {
                transcript_path: m.transcript_path.as_ref().map(|p| pathfmt::from_wsl(&sh, p)),
                cwd: m.cwd.as_ref().map(|p| pathfmt::from_wsl(&sh, p)),
                ..m
            };
            // Konversation und Transkript nur der eigenen Konversation der Sitzung (#302).
            if e.note_conversation(&m, &conv, keyless) {
                self.usage.set_path(m.session, PathBuf::from(&e.meta.transcript));
            }
            if let Some(c) = m.cwd.as_ref().filter(|c| !c.is_empty()) {
                // Linux-Pfad einer WSL-Sitzung als Windows-Pfad wie bei OSC 7 (Dateibaum, Links, #277).
                e.live_cwd = pathfmt::from_wsl(e.eff_shell(), c);
                if e.start_cwd.is_empty() {
                    e.start_cwd = e.live_cwd.clone();
                }
            }
            // Von selbst weiter nur, wenn `take_hook` es erlaubt (der vom Hub gestartete Claude, Ende einer Runde mit Prompt).
            if advance && e.queue_tick(old, new) {
                queue_done.push((m.session, e.display_name()));
            }
            let needs = new != old && matches!(new, ClaudeStatus::Permission | ClaudeStatus::Input);
            let finished = m.event == "Stop" && old == ClaudeStatus::Working && e.status == ClaudeStatus::Idle;
            if !(needs || finished) {
                continue;
            }
            let unseen = !window_focused || focused_id != m.session;
            if !unseen {
                continue;
            }
            e.attention = true;
            let name = e.display_name();
            let body = if needs {
                if e.status_msg.is_empty() { lang.t("Braucht deine Eingabe", "Needs your input").to_string() } else { e.status_msg.clone() }
            } else {
                lang.t("Fertig – wartet auf den nächsten Prompt", "Done – waiting for the next prompt").to_string()
            };
            if !window_focused {
                if self.settings.notify_toast {
                    notify::toast(&name, &body, self.settings.notify_sound);
                }
                if self.settings.notify_flash {
                    ctx.send_viewport_cmd_to(self.attention_viewport(m.session), egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
                }
            }
        }
        self.queue_finished(ctx, queue_done);
        for id in queue_paused {
            if let Some(n) = self.queue_paused_note(id) {
                self.notify_text(n);
            }
        }
    }

    fn queue_finished(&mut self, ctx: &egui::Context, names: Vec<(u64, String)>) {
        let lang = self.settings.language;
        let window_focused = self.seen_focus().0;
        for (sid, name) in names {
            let body = lang.t("Warteschlange abgearbeitet", "Queue finished");
            if window_focused {
                self.notify_text(format!("{name}: {body}"));
            } else {
                if self.settings.notify_toast {
                    notify::toast(&name, body, self.settings.notify_sound);
                }
                if self.settings.notify_flash {
                    ctx.send_viewport_cmd_to(self.attention_viewport(sid), egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
                }
            }
        }
    }

    fn apply(&mut self, ctx: &egui::Context, a: Action) {
        // Sitzung eines anderen Fensters: dort ausführen (das Fenster kommt nach vorn).
        if self.route(ctx, &a) {
            return;
        }
        match a {
            Action::PasteAsk(id, text, detached, check_targets) => self.ask_paste(id, text, detached, check_targets),
            Action::ToggleFiles => self.files_open = !self.files_open,
            Action::Cycle(d) => self.cycle(d),
            Action::JumpCmd(d) => {
                if let Some(sess) = self.entries.get(&self.focused).and_then(|e| e.live.as_ref()) {
                    sess.jump_mark(d);
                }
            }
            Action::ToggleDiff => self.diff_open = !self.diff_open,
            Action::ToggleComposer => {
                self.composer_open = !self.composer_open;
                self.composer_focus = self.composer_open;
            }
            Action::SetProjectProfile(pid, prof) => {
                if let Some(p) = self.projects.iter_mut().find(|p| p.id == pid) {
                    p.profile = prof;
                }
            }
            // Die Warteschlange gibt es nur für Terminals (Claude), nicht für Chats.
            Action::OpenQueue(id) | Action::QueueAdd(id, _) if self.entries.get(&id).is_some_and(|e| e.is_chat()) => {}
            Action::OpenQueue(id) => {
                self.queue_open = if self.queue_open == Some(id) { None } else { Some(id) };
            }
            Action::QueueAdd(id, text) => {
                let text = text.trim().to_string();
                if !text.is_empty() {
                    if let Some(e) = self.entries.get_mut(&id) {
                        e.meta.queue.push(text);
                        // Ist Claude schon bereit, startet die Warteschlange sofort.
                        if e.is_claude() && !e.meta.queue_paused && e.status == ClaudeStatus::Idle && !e.exited() {
                            e.send_next_queued();
                        }
                    }
                }
            }
            Action::Approve(id, yes, seq) => {
                if let Some(e) = self.entries.get_mut(&id) {
                    let settled = e.perm_since.is_none_or(|t| t.elapsed() >= std::time::Duration::from_millis(500));
                    // Nur, wenn die Frage noch genau den Aufruf verlangt, den die Karte zeigte, und der Dialog ihn gerade
                    // zeigt (#390) – sonst nichts senden; die Karte zeigt bis zur nächsten Prüfung nur *Öffnen*.
                    let screen = e.live.as_ref().map(|s| s.screen_text()).unwrap_or_default();
                    if !e.answerable(seq, &screen) {
                        if let Some(r) = e.track.request.as_mut() {
                            r.confirmed = false;
                        }
                    } else if e.status == ClaudeStatus::Permission && settled && !e.exited() {
                        e.track.tools.answered();
                        if let Some(s) = &e.live {
                            s.write(if yes { b"\r" } else { b"\x1b" });
                        }
                        if yes {
                            e.assume_working();
                        } else {
                            // Abgelehnt: Claude bricht die Runde ab und wartet (dafür gibt es keinen Stop-Hook). Der
                            // abgelehnte Aufruf meldet kein PostToolUse mehr (#390).
                            e.track.tools.forget(seq);
                            e.apply_fallback(ClaudeStatus::Idle);
                        }
                        e.clear_status_msg();
                        e.perm_since = None;
                        e.attention = false;
                    }
                }
            }
            Action::OpenGlobalSearch => self.gsearch = Some(GlobalSearch::default()),
            Action::GlobalJump(sid, line, query) => {
                if let Some(pid) = self.entries.get(&sid).map(|e| e.meta.project) {
                    self.switch_project(pid);
                }
                if self.detached.contains(&sid) {
                    ctx.send_viewport_cmd_to(egui::ViewportId::from_hash_of(("detached", sid)), egui::ViewportCommand::Focus);
                } else {
                    self.activate(sid);
                }
                if let Some(s) = self.entries.get_mut(&sid).and_then(|e| e.live.as_mut()) {
                    let mut st = SearchState::new();
                    st.focus_input = false;
                    st.query = query.clone();
                    st.last_query = query;
                    let activity = s.output_stamp();
                    let mut t = s.term.lock().unwrap_or_else(|e| e.into_inner());
                    st.compute(&t, s.hist_base(), activity);
                    st.refreshed = ctx.input(|i| i.time);
                    st.current = st.matches.iter().position(|m| m.line == line).unwrap_or(st.matches.len().saturating_sub(1));
                    search::scroll_to(&mut t, line);
                    drop(t);
                    s.search = Some(st);
                }
            }
            Action::ExportHistory(id) => self.export_history(id),
            Action::OpenEditor => {
                if self.editor.is_some() {
                    self.editor = None;
                } else {
                    let dir = self.entries.get(&self.focused).map(|e| e.cwd_now()).unwrap_or_else(home_dir);
                    self.editor = Some(Editor::new(&dir));
                }
            }
            Action::InsertPath(p) => {
                let mut refused = None;
                if let Some(e) = self.entries.get_mut(&self.focused) {
                    // Chat: Pfad unverändert ins Eingabefeld; Terminal: für die Shell umgerechnet und gequotet (#392:
                    // `\\wsl.localhost\<D>\x` → `/x`, `C:\x` → `/mnt/c/x`); ein Pfad einer anderen Distribution nicht.
                    let text = match &e.live {
                        Some(s) => match crate::targetpath::for_insert(&s.spec, &p) {
                            Ok(t) => Some(format!("{t} ")),
                            Err(err) => {
                                refused = Some(err);
                                None
                            }
                        },
                        None => Some(p.clone()),
                    };
                    if let Some(text) = text {
                        e.insert_text(&text, false);
                    }
                }
                if let Some(err) = refused {
                    self.path_refused_toast(&err);
                }
                self.want_focus = Some(self.focused);
            }
            Action::NewSessionAt(dir) => {
                // Wie „Hier öffnen“ (#392): ein WSL-Ordner bekommt die WSL-Shell seiner Distribution.
                self.open_folder(ctx, &dir, false, None);
            }
            Action::OpenPalette => self.palette = Some(Palette { query: String::new(), sel: 0 }),
            Action::EditorFormat(op) => self.editor_format_active(ctx, op),
            Action::SaveActions(c) => self.sa_palette(c),
            Action::Editor(a) => self.editor_run_action(ctx, a),
            Action::OpenHistory => {
                self.history_open = true;
                if self.history_rx.is_none() {
                    self.history_rx = Some(history::scan_async(ctx.clone()));
                }
            }
            Action::ResumeHistory(i) => {
                if let Some(h) = self.history.get(i).cloned() {
                    self.apply(ctx, Action::ResumeItem(h));
                }
            }
            Action::ResumeItem(h) => {
                let lang = self.settings.language;
                // Der Verlauf liest nur Windows-Konversationen (`~/.claude`); eine Datei ohne Konversations-ID wäre still
                // als neue Konversation gestartet worden (#245).
                if !crate::quote::is_uuid(&h.session_id) {
                    self.notify_text(lang.t("Diese Konversation lässt sich nicht fortsetzen (keine gültige Konversations-ID).", "This conversation cannot be resumed (no valid conversation id).").to_string());
                    return;
                }
                // Schon in einer Sitzung offen: dorthin wechseln statt sie ein zweites Mal zu öffnen (#245).
                if let Some(open) = self.entries.iter().find(|(_, e)| e.launched_claude && !e.exited() && e.meta.claude_session == h.session_id).map(|(id, _)| *id) {
                    self.notify_text(lang.t("Die Konversation ist schon in einer Sitzung offen.", "The conversation is already open in a session.").to_string());
                    self.apply(ctx, Action::Activate(open));
                    self.history_open = false;
                    return;
                }
                {
                    let name: String = h.title.chars().take(32).collect();
                    self.new_session(
                        ctx,
                        NewOpts {
                            // Windows-Konversation: in einer Windows-Shell fortsetzen, auch wenn WSL die Standard-Shell ist.
                            shell_id: self.windows_shell_id(),
                            profile: self.windows_shell_id().map(|_| 0),
                            cwd: Some(h.cwd.clone()),
                            name: Some(name),
                            claude: true,
                            resume_id: Some(h.session_id.clone()),
                            transcript: h.path.to_string_lossy().to_string(),
                            ..NewOpts::default()
                        },
                    );
                    if let Some(t) = self.entries.get(&self.focused).map(|e| e.meta.transcript.clone()).filter(|t| !t.is_empty()) {
                        self.usage.set_path(self.focused, PathBuf::from(t));
                    }
                    self.history_open = false;
                }
            }
            Action::OpenSettings => self.settings_open = true,
            Action::OpenCleanup => self.open_cleanup(ctx),
            Action::OpenPrompts => self.prompts_open = true,
            Action::NewDialog(claude) => {
                // Aus der Palette: den Fokus nicht ans Terminal zurückgeben, er gehört dem Dialog (#255).
                self.want_focus = None;
                self.new_dialog = Some(NewDialog::new(self.settings.default_shell.clone(), claude && self.settings.claude_mode, self.dialog_claude_opts()));
            }
            Action::OpenNotes(id) => {
                self.notes_open = if self.notes_open == Some(id) { None } else { Some(id) };
            }
            Action::ResetAgentChain(id) => {
                if taint::reset(&mut self.entries, id) {
                    self.save_state();
                }
            }
            Action::LinkTicket(id) => self.tlinks_open_picker(id),
            Action::OpenTicket(id) => self.open_ticket_from_anywhere(id),
            Action::UnlinkTicket(session, ticket) => self.tlinks_unlink(ticket, session),
            Action::OpenSearch => {
                if let Some(s) = self.entries.get_mut(&self.focused).and_then(|e| e.live.as_mut()) {
                    if s.search.is_some() {
                        s.search = None;
                        self.want_focus = Some(self.focused);
                    } else {
                        s.search = Some(SearchState::new());
                    }
                }
            }
            Action::ToggleBroadcast => {
                self.broadcast = !self.broadcast;
                self.want_focus = Some(self.focused);
            }
            Action::SendSnippet(i) => {
                // Chat: Text ins Eingabefeld (nicht abgeschickt); Terminal: wie bisher, ggf. mit Enter.
                if let (Some(sn), Some(e)) = (self.prompts.get(i), self.entries.get_mut(&self.focused)) {
                    e.insert_text(&sn.text, sn.submit);
                }
                self.want_focus = Some(self.focused);
            }
            Action::FocusDir(nav) => {
                if let Some(id) = focus_target(&self.pane_rects, self.focused, nav) {
                    self.activate(id);
                }
            }
            Action::Detach(id) => {
                if self.entries.contains_key(&id) && !self.detached.contains(&id) {
                    self.hide_pane(id);
                    self.detached.push(id);
                    // In einem eigenen Fenster ist sie sichtbar, also nicht mehr minimiert.
                    if let Some(e) = self.entries.get_mut(&id) {
                        e.meta.minimized = false;
                        e.meta.slot = None;
                    }
                }
            }
            Action::Attach(id) => {
                self.detached.retain(|d| *d != id);
                if self.entries.get(&id).is_some_and(|e| e.meta.project != self.active_project) {
                    if let Some(p) = self.entries.get(&id).map(|e| e.meta.project) {
                        self.switch_project(p);
                    }
                }
                let ok = match &mut self.layout {
                    Some(l) if !l.contains(id) => l.split_leaf_at(self.focused, id, Dir::Horizontal, false),
                    _ => false,
                };
                if ok {
                    self.focused = id;
                    self.want_focus = Some(id);
                } else {
                    self.activate(id);
                }
            }
            Action::SwitchProject(id) => self.switch_project(id),
            Action::NewProjectPrompt => self.text_prompt = Some(TextPrompt { kind: PromptKind::NewProject, id: 0, tags_for: None, text: String::new(), folder: String::new(), picker: None }),
            Action::ProjectPrompt(id, kind) => {
                let text = self
                    .projects
                    .iter()
                    .find(|p| p.id == id)
                    .map(|p| if kind == PromptKind::ProjectFolder { p.folder.clone() } else { p.name.clone() })
                    .unwrap_or_default();
                // Standardordner eines Projekts, dessen Shell WSL ist: Ordnerfeld mit Linux-Pfad (#172).
                let picker = (kind == PromptKind::ProjectFolder)
                    .then(|| self.folder_target(self.projects.iter().find(|p| p.id == id).map(|p| p.profile).unwrap_or(0)))
                    .filter(|t| *t != folder_picker::Target::Windows)
                    .map(|t| folder_picker::FolderPicker::new(("projfolder", id), &text, t));
                self.text_prompt = Some(TextPrompt { kind, id, tags_for: None, text, folder: String::new(), picker });
            }
            Action::SetProjectColor(id, c) => {
                if let Some(p) = self.projects.iter_mut().find(|p| p.id == id) {
                    p.color = c;
                }
            }
            Action::TagsPrompt(id) => {
                let text = self.entries.get(&id).map(|e| e.meta.tags.join(", ")).unwrap_or_default();
                self.text_prompt = Some(TextPrompt { kind: PromptKind::NewProject, id: 0, tags_for: Some(id), text, folder: String::new(), picker: None });
            }
            Action::DeleteProjectAsk(id) => {
                if self.projects.len() > 1 {
                    self.want_focus = None;
                    self.confirm_close = Some(closing::CloseAsk::new(closing::CloseTarget::DeleteProject(id)));
                }
            }
            Action::DeleteProject(id) => {
                let other = self.projects.iter().find(|p| p.id != id).map(|p| p.id);
                if let (true, Some(other)) = (self.projects.len() > 1, other) {
                    // Hinweis, wenn Agenten durch das Verschieben mehr Zugriff bekommen (#386) – vor dem Entfernen prüfen,
                    // auch wenn das Ziel nur für sein eigenes Projekt (ohne „Für alle“) freigegeben ist.
                    let gains = self.access_map(self.settings.mcp_level).move_gains(id, other);
                    for e in self.entries.values_mut() {
                        if e.meta.project == id {
                            e.meta.project = other;
                        }
                    }
                    // Karten, verknüpfte Ticket-Projekte und Prozesse ziehen mit; es gilt sofort der Zugriff des Ziels (#386).
                    if let Some(db) = self.db.as_ref() {
                        let _ = crate::planning::access::sidebar_project_moved(&db.conn, id, other);
                        self.tasks.invalidate();
                    }
                    if crate::procs::move_project(&mut self.procs.defs, id, other) {
                        self.procs_save();
                    }
                    if gains {
                        let name = self.projects.iter().find(|p| p.id == other).map(|p| p.name.clone()).unwrap_or_default();
                        let lang = self.settings.language;
                        let text = if lang == Lang::De {
                            format!("Hinweis: In „{name}“ haben Agenten der verschobenen Sitzungen mehr Zugriff als vorher – das gilt sofort.")
                        } else {
                            format!("Note: agents of the moved sessions have more access in “{name}” than before – this applies immediately.")
                        };
                        self.push_toast(text, toasts::ToastKind::Info, None);
                    }
                    if self.active_project == id {
                        self.projects.retain(|p| p.id != id);
                        self.active_project = other;
                        if let Some(p) = self.projects.iter().find(|p| p.id == other).cloned() {
                            self.load_project_view(p.layout, p.focused);
                        }
                    } else {
                        self.projects.retain(|p| p.id != id);
                    }
                }
            }
            Action::Activate(id) | Action::Focus(id) if self.detached.contains(&id) => {
                ctx.send_viewport_cmd_to(egui::ViewportId::from_hash_of(("detached", id)), egui::ViewportCommand::Focus);
            }
            Action::Activate(id) | Action::Focus(id) => {
                // Eine Sitzung nach vorn: die Ausgabe eines Prozesses (#243) macht ihr Platz.
                self.procs.view = None;
                if let Some(pid) = self.entries.get(&id).map(|e| e.meta.project) {
                    self.switch_project(pid);
                }
                self.activate(id)
            }
            // Abgedockt: schon in seinem eigenen Fenster zu sehen; nichts nach vorn holen (Fokus des Nutzers bleibt).
            Action::Place(id, ..) if self.detached.contains(&id) => {}
            Action::Place(id, placement, caller, protected) => self.place(id, placement, caller, &protected),
            Action::JumpTo(id) => {
                if self.entries.contains_key(&id) {
                    self.set_mode(titlebar::Mode::Terminals);
                    self.apply(ctx, Action::Activate(id));
                }
            }
            Action::StartRename(id) => {
                let target = self.entries.get(&id).map(|e| (self.in_win(e), self.sidebar_match(e), e.meta.window, e.meta.project));
                match target {
                    None => {}
                    Some((false, _, window, _)) => {
                        // Sitzung eines anderen Fensters (Palette): dort umbenennen; das Fenster kommt dabei nach vorn
                        // (Review zu #259).
                        self.route_to(ctx, window, Action::StartRename(id));
                    }
                    Some((true, matched, _, project)) => {
                        if !matched && project != self.active_project {
                            // Anderes Projekt: dorthin wechseln, damit die Zeile erscheint. Der Fokus bleibt beim
                            // Umbenennen-Feld (ein Terminal, das ihn danach nähme, beendete das Umbenennen sofort).
                            self.filter.clear();
                            self.switch_project(project);
                            self.want_focus = None;
                        }
                        // Die Zeile muss zu sehen sein (Filter, eingeklappter Ordner), sonst geschähe nichts Sichtbares und
                        // das wartende Umbenennen holte sich später den Fokus vom Terminal (#259).
                        if self.reveal_row(id) {
                            if let Some(e) = self.entries.get(&id) {
                                self.rename = Some((id, e.display_name()));
                            }
                        } else {
                            let lang = self.settings.language;
                            self.notify_text(lang.t("Diese Sitzung lässt sich hier nicht umbenennen.", "This session can't be renamed here.").to_string());
                        }
                    }
                }
            }
            Action::SetColor(id, c) => {
                if let Some(e) = self.entries.get_mut(&id) {
                    e.meta.color = c;
                }
            }
            Action::SetGroup(id, g) => self.set_group(id, g),
            Action::OpenTemplates => {
                self.want_focus = None;
                self.templates_open = true;
            }
            Action::OpenCosts => self.cost_open = true,
            Action::TemplateDeleteAsk(i) => {
                if let Some(t) = self.templates.get(i) {
                    self.confirm_close = Some(closing::CloseAsk::new(closing::CloseTarget::DeleteTemplate(i, t.name.clone())));
                }
            }
            Action::TemplateDelete(i, name) => {
                // Nur, wenn an dieser Stelle noch genau diese Vorlage steht (die Liste kann sich verschoben haben).
                if self.templates.get(i).is_some_and(|t| t.name == name) {
                    self.templates.remove(i);
                    templates::save(&self.dir, &self.templates);
                }
            }
            Action::TemplateSave(name, folder) => {
                let pid = self.active_project;
                let mut ids: Vec<u64> = self
                    .entries
                    .iter()
                    // Vorlagen kennen nur Terminals.
                    .filter(|(_, e)| !e.is_chat() && e.meta.project == pid && self.in_win(e) && folder.as_ref().is_none_or(|f| !e.meta.group.is_empty() && state::folder_within(&e.meta.group, f)))
                    .map(|(i, _)| *i)
                    .collect();
                ids.sort();
                let items: Vec<TplItem> = ids.iter().filter_map(|i| self.entries.get(i)).map(|e| self.tpl_item_of(e)).collect();
                let lang = self.settings.language;
                if items.is_empty() {
                    self.notify_text(lang.t("Keine Sitzungen zum Speichern", "No sessions to save").to_string());
                } else {
                    let n = items.len();
                    let name = if name.trim().is_empty() { format!("{} {}", lang.t("Vorlage", "Template"), self.templates.len() + 1) } else { name.trim().to_string() };
                    // Gleichnamige Gruppen-Vorlage (ohne Groß-/Kleinschreibung) ersetzen, nie still eine Terminal-Vorlage.
                    let name = templates::save_group(&mut self.templates, Template { name, grid: n > 1, items, terminal: false });
                    templates::save(&self.dir, &self.templates);
                    self.notify_text(format!("{} {} ({} {})", lang.t("Vorlage gespeichert:", "Template saved:"), lang.q(&name), n, lang.t("Sitzungen", "sessions")));
                }
            }
            Action::TemplateStart(i) => {
                self.start_template(ctx, i, tpl_lib::TplAt::Default);
            }
            Action::TemplateStartAt(i, at) => {
                self.start_template(ctx, i, at);
            }
            Action::TemplateEdit(i) => {
                if let Some(t) = self.templates.get(i) {
                    self.want_focus = None;
                    self.tpl.form = Some(tpl_lib::TplForm::of(Some(i), t));
                }
            }
            Action::TemplateNew => {
                self.want_focus = None;
                let mut f = tpl_lib::TplForm::blank();
                f.item.claude = false;
                self.tpl.form = Some(f);
            }
            Action::TemplateDuplicate(i) => {
                if let Some(mut t) = self.templates.get(i).cloned() {
                    let lang = self.settings.language;
                    t.name = templates::unique_name(&format!("{} {}", t.name.trim(), lang.t("(Kopie)", "(copy)")), &self.templates);
                    let name = t.name.clone();
                    self.templates.insert(i + 1, t);
                    templates::save(&self.dir, &self.templates);
                    self.notify_text(format!("{} {}", lang.t("Vorlage dupliziert:", "Template duplicated:"), lang.q(&name)));
                }
            }
            Action::TemplateFromSession(id) => {
                if let Some(e) = self.entries.get(&id).filter(|e| !e.is_chat()) {
                    let mut it = self.tpl_item_of(e);
                    let name = e.display_name();
                    // Nur ein selbst vergebener Name wird zum Sitzungsnamen; sonst gilt der Name der Vorlage.
                    if !e.meta.custom_name {
                        it.name = String::new();
                    }
                    // Geheime Werte (#384) kommen nur nach Rückfrage in die Vorlage (Haken im Reiter „Umgebung“).
                    let secrets = e.meta.env.iter().any(|v| v.secret && v.value_protected.is_some());
                    it.env = e.meta.env.clone();
                    let base = templates::unique_name(&name, &self.templates);
                    self.want_focus = None;
                    let mut f = tpl_lib::TplForm::of(None, &Template::terminal(&base, it));
                    if secrets {
                        f.env.keep_secrets = Some(false);
                    }
                    self.tpl.form = Some(f);
                }
            }
            Action::ToggleTree => {
                let pid = self.active_project;
                if let Some(p) = self.projects.iter_mut().find(|p| p.id == pid) {
                    p.tree_view = !p.tree_view;
                }
            }
            Action::FolderNew(parent) => self.folder_prompt = Some(FolderPrompt { kind: 0, path: parent, text: String::new() }),
            Action::FolderRename(path) => {
                let text = state::folder_leaf(&path).to_string();
                self.folder_prompt = Some(FolderPrompt { kind: 1, path, text });
            }
            Action::MoveFolder(path, parent) => {
                let new = if parent.is_empty() { state::folder_leaf(&path).to_string() } else { format!("{}/{}", parent, state::folder_leaf(&path)) };
                if new != path && !self.rename_folder_to(&path, &new) {
                    let lang = self.settings.language;
                    self.notify_text(lang.t("Verschieben nicht möglich (Name existiert schon oder Ziel liegt im Ordner selbst)", "Cannot move (name already exists or target is inside the folder)").to_string());
                }
            }
            Action::FolderTemplate(path) => self.folder_prompt = Some(FolderPrompt { kind: 3, text: state::folder_leaf(&path).to_string(), path }),
            Action::FolderBroadcast(path) => {
                self.want_focus = None;
                self.folder_prompt = Some(FolderPrompt { kind: 2, path, text: String::new() });
            }
            // Ordner löschen (#276): Rückfrage mit Wartezeit statt „zweimal klicken“ (`closing`).
            Action::FolderDeleteAsk(path) => {
                self.want_focus = None;
                self.ask_folder_lift(path);
            }
            Action::FolderDelete(pid, path) => self.lift_folder(pid, &path),
            Action::FolderRemove(pid, path) => self.remove_folder_tree(pid, &path),
            Action::FolderToggle(path) => {
                if let Some(f) = self.active_folder_mut(&path) {
                    f.collapsed = !f.collapsed;
                }
            }
            Action::FolderColor(path, c) => {
                if let Some(f) = self.active_folder_mut(&path) {
                    f.color = c;
                }
            }
            Action::FolderCwd(path, clear) => {
                // Shell des Ordners (geerbtes Profil, sonst das des Projekts) ist WSL: Ordnerfeld mit Linux-Pfad (#172).
                let proj = self.projects.iter().find(|p| p.id == self.active_project);
                let pid = proj.and_then(|p| p.inherited(&path, |f| Some(f.profile).filter(|x| *x != 0)).or(Some(p.profile))).unwrap_or(0);
                let target = self.folder_target(pid);
                if !clear && target != folder_picker::Target::Windows {
                    let cur = proj.and_then(|p| p.folder(&path)).map(|f| f.cwd.clone()).unwrap_or_default();
                    let picker = folder_picker::FolderPicker::new(("foldercwd", &path), &cur, target);
                    self.text_prompt = Some(TextPrompt { kind: PromptKind::FolderCwd, id: self.active_project, tags_for: None, text: cur, folder: path, picker: Some(picker) });
                    return;
                }
                let start = self.entries.get(&self.focused).map(|e| e.cwd_now()).unwrap_or_else(home_dir);
                let new = if clear { Some(String::new()) } else { notify::pick_folder(&start) };
                if let (Some(c), Some(f)) = (new, self.active_folder_mut(&path)) {
                    f.cwd = c;
                }
            }
            Action::FolderProfile(path, prof) => {
                if let Some(f) = self.active_folder_mut(&path) {
                    f.profile = prof;
                }
            }
            Action::FolderNewSession(path, claude) => {
                let proj = self.projects.iter().find(|p| p.id == self.active_project);
                let cwd = proj.and_then(|p| p.inherited(&path, |f| Some(f.cwd.clone()).filter(|c| !c.is_empty())));
                let prof = proj.and_then(|p| p.inherited(&path, |f| Some(f.profile).filter(|x| *x != 0)));
                let mut d = NewDialog::new(self.settings.default_shell.clone(), claude && self.settings.claude_mode, self.dialog_claude_opts());
                if let Some(c) = cwd.or_else(|| proj.map(|p| p.folder.clone()).filter(|c| !c.is_empty())) {
                    d.set_cwd(&c);
                }
                if let Some(pr) = prof {
                    d.profile = pr;
                }
                d.group = path;
                self.new_dialog = Some(d);
            }
            Action::FolderRestartAll(path) => {
                // Wie „Neu laden“ (#213): Hat eine beendete Sitzung noch Hintergrundprozesse im Job, wird erst gefragt
                // (eine Rückfrage zur Zeit; weitere solche Sitzungen bleiben und werden gemeldet).
                let mut skipped = 0;
                for id in self.folder_sessions(&path) {
                    if !self.entries.get(&id).is_some_and(|e| !e.is_chat() && e.exited()) {
                        continue;
                    }
                    if self.lingering_of(&[id]).is_empty() {
                        self.apply(ctx, Action::Restart(id));
                    } else if self.reload_ask.is_none() {
                        self.request_reload(ctx, id);
                    } else {
                        skipped += 1;
                    }
                }
                if skipped > 0 {
                    let lang = self.settings.language;
                    self.notify_text(match lang {
                        Lang::De => format!("{skipped} weitere beendete Sitzung(en) mit Hintergrundprozessen nicht neu gestartet – bitte einzeln neu starten."),
                        Lang::En => format!("{skipped} more ended session(s) with background processes not restarted – please restart them one by one."),
                    });
                }
            }
            Action::CloseAll(scope) => self.request_close_many(scope),
            Action::CloseMany(ids) => self.close_many(&ids),
            Action::FolderLayout(path) => {
                let ids: Vec<u64> = self.folder_sessions(&path).into_iter().filter(|id| self.entries.get(id).is_some_and(|e| !e.exited())).take(12).collect();
                if let Some(l) = Node::grid(&ids, Dir::Horizontal) {
                    self.detached.retain(|d| !ids.contains(d));
                    self.layout = Some(l);
                    self.focused = ids[0];
                    self.want_focus = Some(ids[0]);
                }
            }
            Action::RequestReload(id) => self.request_reload(ctx, id),
            Action::ConvPickFresh(id) => self.pick_fresh(ctx, id),
            Action::ConvPickShow => self.show_pick_window(ctx),
            Action::OpenSessionSettings(id) => self.open_session_settings(id),
            Action::Restart(id) if self.entries.get(&id).is_some_and(|e| e.is_chat()) => {} // ein Chat hat keinen Prozess
            Action::Restart(id) => {
                if let Some(mut e) = self.entries.remove(&id) {
                    // Alten Job mit übrigen Prozessen behalten (#218, #213), dann erst die alte Shell beenden.
                    let name = e.display_name();
                    if let Some(s) = e.live.as_mut() {
                        self.retire_job(id, name, s);
                    }
                    e.live = None;
                    // „Konversation auswählen“ (#302): erst die Auswahl, dann der Start (`conv_pick`).
                    if !self.begin_pick(ctx, &mut e, false, true) {
                        self.spawn_into(ctx, &mut e, Launch { restore: false, restart: true, continue_last: false, prompt: None, resume_id: None });
                    }
                    self.entries.insert(id, e);
                }
            }
            Action::EnvCopy(id) => self.env_copy(id),
            Action::EnvPaste(id) => self.env_paste(id),
            Action::EnvCopyDotenv(id) => self.env_copy_dotenv(ctx, id),
            Action::EnvUnlock(id) => {
                // Gesperrte geheime Variablen (#384): erneut starten – unter Linux/macOS fragt das Fenster dabei nach dem
                // Master-Passwort; lassen sich die Werte nicht entschlüsseln, die Einstellungen der Sitzung öffnen.
                let reentry = self.entries.get(&id).and_then(|e| e.env_locked.as_ref()).is_some_and(|l| l.needs_reentry());
                if reentry {
                    self.apply(ctx, Action::EnvEdit(id));
                } else if self.entries.get(&id).is_some_and(|e| e.live.is_none()) {
                    self.apply(ctx, Action::Restart(id));
                }
            }
            Action::EnvEdit(id) => {
                self.open_session_settings(id);
                if let Some(f) = self.session_settings.as_mut() {
                    f.start_tab = Some(opts_tabs::Tab::Environment);
                }
            }
            Action::Duplicate(id) => {
                if let Some(src) = self.entries.get(&id) {
                    if src.is_chat() {
                        // Chat: neuer, leerer Chat mit demselben Modell im selben Ordner (ohne Modell: Dialog).
                        let Some(chat) = src.meta.chat.clone() else {
                            let mut d = models_ui::NewChatDlg::new(&self.settings, Some(src.meta.cwd.clone()), Some(src.meta.group.clone()));
                            d.window = Some(src.meta.window);
                            d.project = Some(src.meta.project);
                            self.new_chat = Some(d);
                            return;
                        };
                        let o = NewOpts {
                            cwd: Some(src.meta.cwd.clone()),
                            group: Some(src.meta.group.clone()),
                            window: Some(src.meta.window),
                            project: Some(src.meta.project),
                            chat: Some(chat),
                            ..NewOpts::default()
                        };
                        self.new_session(ctx, o);
                        return;
                    }
                    let o = NewOpts {
                        shell_id: Some(src.meta.shell_id.clone()),
                        cwd: Some(src.meta.cwd.clone()),
                        name: Some(format!("{} (2)", src.display_name())),
                        claude: src.meta.claude && self.settings.claude_mode,
                        claude_args: src.meta.claude_args.clone(),
                        resume_mode: Some(src.meta.resume_mode()),
                        claude_opts: Some(src.meta.claude_opts.clone()),
                        profile: Some(src.meta.profile),
                        window: Some(src.meta.window),
                        project: Some(src.meta.project),
                        // Duplizieren kopiert die Umgebungsvariablen (#384), danach unabhängig.
                        env: src.meta.env.clone(),
                        ..NewOpts::default()
                    };
                    let new = self.new_session(ctx, o);
                    let group = self.entries.get(&id).map(|e| e.meta.group.clone()).unwrap_or_default();
                    if let Some(e) = self.entries.get_mut(&new) {
                        e.meta.group = group;
                    }
                }
            }
            Action::RequestClose(id) if self.entries.get(&id).is_some_and(|e| e.meta.worktree.is_some()) => {
                // `git status` im Hintergrund: in großen oder langsamen Repos fröre sonst jedes Fenster ein (#259). Bis das
                // Ergebnis da ist, zeigt die Rückfrage „wird geprüft“ und bietet nur Behalten/Abbrechen an.
                let path = self.entries.get(&id).and_then(|e| e.meta.worktree.as_ref()).map(|w| PathBuf::from(&w.path)).unwrap_or_default();
                let (tx, rx) = std::sync::mpsc::channel();
                let ctx2 = ctx.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(gitops::status(&path).map(|c| c.len()).unwrap_or(0));
                    ctx2.request_repaint();
                });
                // Hintergrundprozesse im Job (#213): auch hier nennen; sie können den Ordner belegt halten.
                let lingering = self.lingering_of(&[id]);
                self.wt_close = Some(WtClose { id, changes: 0, changes_rx: Some(rx), discard: false, lingering, end_jobs: false });
            }
            Action::RequestClose(id) => {
                // Chat: nachfragen, wenn dabei ein Verlauf gelöscht würde (oder eine Antwort läuft). Hintergrundprozesse
                // im Job der Sitzung (#168): immer nachfragen (weiterlaufen lassen oder alle beenden).
                let running = self.entries.get(&id).is_some_and(|e| e.running() || e.chat_has_history());
                let lingering = self.lingering_of(&[id]);
                if !lingering.is_empty() || (running && self.settings.confirm_close) {
                    self.confirm_close = Some(closing::CloseAsk::new(closing::CloseTarget::One(id)).with(lingering));
                } else {
                    self.close_session(id);
                }
            }
            Action::Close(id) => self.close_session(id),
            Action::EndJobs(ids) => self.end_jobs(&ids),
            Action::Split(id, dir) => self.split_at(ctx, id, dir, false),
            Action::SplitAt(id, dir, before) => self.split_at(ctx, id, dir, before),
            Action::MinimizePane(id) => self.minimize_pane(id),
            Action::NewQuick => {
                self.new_session(ctx, NewOpts::default());
            }
            Action::NewChat => {
                self.new_chat = Some(models_ui::NewChatDlg::new(&self.settings, None, None));
            }
            Action::FolderNewChat(path) => {
                let proj = self.projects.iter().find(|p| p.id == self.active_project);
                let cwd = proj.and_then(|p| p.inherited(&path, |f| Some(f.cwd.clone()).filter(|c| !c.is_empty())));
                self.new_chat = Some(models_ui::NewChatDlg::new(&self.settings, cwd, Some(path)));
            }
            Action::ChatSend(id) | Action::ChatStop(id) | Action::ChatRegenerate(id) | Action::ChatEdit(id) | Action::ChatDecide(id, ..) | Action::ChatContinue(id) | Action::ChatRevoke(id, _) | Action::ChatLevel(id, _) | Action::ChatCompact(id) => self.chat_action(ctx, id, &a),
            Action::ChatPlan(id, act) => self.chat_plan_action(ctx, id, &act),
            Action::ChatImage(id, req) => self.chat_image(ctx, id, req),
            Action::Tutorial(c) => self.tut_cmd(ctx, c),
        }
    }
}

/// Wie `consume_key`, aber mit genau diesen Modifikatoren (#254): ein zusätzliches Umschalt oder Alt (AltGr = Strg+Alt)
/// ist eine andere Kombination – sonst löste Strg+Umschalt+1 das Kürzel Strg+1 aus und Strg+AltGr+2 („²“) Strg+2.
/// `physical`: auch die physische Taste erkennen – mit Umschalt liefert die Tastatur eine andere logische Taste
/// (Umschalt+1 = „!“ bzw. „/“ je nach Belegung), `consume_key` sähe dann nicht `Key::Num1`.
fn consume_key_exact(i: &mut egui::InputState, m: Modifiers, k: Key, physical: bool) -> bool {
    let before = i.events.len();
    i.events.retain(|e| {
        !matches!(e, Event::Key { key, physical_key, pressed: true, modifiers, .. }
            if (*key == k || (physical && *physical_key == Some(k))) && modifiers.matches_exact(m))
    });
    i.events.len() != before
}

/// Nächster Bereich in Richtung `nav` vom fokussierten aus (Strg+Alt+Pfeil); `None`, wenn dort keiner liegt.
fn focus_target(rects: &HashMap<u64, Rect>, focused: u64, nav: Nav) -> Option<u64> {
    let c = rects.get(&focused)?.center();
    rects
        .iter()
        .filter(|(id, _)| **id != focused)
        .filter_map(|(id, r)| {
            let d = r.center() - c;
            let (along, across) = match nav {
                Nav::Left => (-d.x, d.y.abs()),
                Nav::Right => (d.x, d.y.abs()),
                Nav::Up => (-d.y, d.x.abs()),
                Nav::Down => (d.y, d.x.abs()),
            };
            (along > 1.0).then_some((*id, along + 2.0 * across))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, _)| id)
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::TRANSPARENT.to_array()
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.sync_accent();
        self.ensure_style(ctx);
        self.apply_font(ctx);
        // Eigener Hintergrund des Themes (#387): zuerst, damit er unter allen Flächen liegt.
        self.backdrop_frame(ctx);
        self.backdrop_tick(self.theme_ui.editor.is_some());
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
        if (ctx.zoom_factor() - self.settings.ui_scale).abs() > 0.001 {
            ctx.set_zoom_factor(self.settings.ui_scale);
        }
        #[cfg(feature = "readme-shots")]
        self.dev_shot_mode(ctx);
        // Tutorial (#143): Ziele des vorigen Bilds verwerfen, Esc für die laufende Tour abfangen.
        self.tut_begin(ctx);
        self.track_focus(ctx);
        for n in self.spawn_notes.take() {
            self.notify_text(n);
        }
        self.handle_ipc(ctx);
        // Hinweise vom Start einer Sitzung (weggelassene Pfade der Claude-Optionen, #182).
        let notes: Vec<String> = self.entries.values_mut().filter_map(|e| e.launch_note.take()).collect();
        for n in notes {
            self.notify_text(n);
        }
        self.poll_mcp(ctx);
        self.editor_tick(ctx);
        self.poll_procs(ctx);
        self.poll_save_actions(ctx);
        // Ordnerfelder ohne eigene Shell (Arbeitsordner der Planung) folgen der Standard-Shell (#392).
        folder_picker::set_default_target(folder_picker::Target::for_shell(self.shell_by_id(&self.effective_shell_id(None, self.active_project)).as_ref(), false));
        self.poll_editor_services(ctx);
        self.poll_retired(ctx);
        // Linux/macOS: Master-Passwort für API-Schlüssel, wenn ein Zugriff darauf wartet (#149).
        #[cfg(unix)]
        self.master_password_ui(ctx);
        crate::platform::conpty::USE_BUNDLED.store(self.settings.bundled_conpty, std::sync::atomic::Ordering::Relaxed);
        termview::CURSOR_BLINK.store(self.settings.cursor_blink, std::sync::atomic::Ordering::Relaxed);
        if crate::platform::conpty::take_fallback_hint() {
            self.push_toast(
                state::t(
                    "Der mitgelieferte Konsolen-Host (OpenConsole) ließ sich mehrmals nicht starten – neue Sitzungen laufen auf dem Konsolen-Host von Windows. Ein Virenscanner kann OpenConsole.exe blockieren; abschalten lässt er sich unter Einstellungen → Darstellung.",
                    "The bundled console host (OpenConsole) failed to start several times – new sessions run on the console host built into Windows. An antivirus may block OpenConsole.exe; it can be turned off under Settings → Appearance.",
                )
                .to_string(),
                toasts::ToastKind::Warning,
                None,
            );
        }
        crate::usage::set_prices(self.settings.prices.clone());
        // Paletten für alle Farbauswahlen und die automatische Vergabe (auch Todo-Tags, Expertenmodus, Diagramme).
        crate::palette::publish(&self.settings.palette);
        self.handle_tray(ctx);
        self.quit_guard(ctx);

        self.poll_hooks(ctx);
        self.poll_shell(ctx);
        self.claude_status_tick(ctx);
        self.poll_vt(ctx);
        self.poll_chats(ctx);
        self.models_save_tick(ctx);
        self.poll_background(ctx);
        self.sync_watchers(ctx);
        self.tlinks_frame(ctx);
        self.poll_history();
        self.poll_picks(ctx);
        self.check_budget(ctx);
        // Reiter „Zeit“ (#164): Wecker, Timer, Geburtstage und Benachrichtigungen – auch minimiert und im Tray.
        self.time_tick(ctx);
        self.handle_drops(ctx);
        let mut actions: Vec<Action> = self.main_frame_begin(ctx);
        // Reiter der Sitzungsdialoge (#292): Standards zum Zählen/Zurücksetzen; Tasten zum Wechseln der Reiter gehören
        // dem offenen Dialog, nicht den globalen Kürzeln.
        ctx.data_mut(|d| d.insert_temp(Id::new("claude_user_defaults"), self.settings.claude_defaults.sanitized(false)));
        // Nur der Dialog, der gerade die Tastatur hat (Fokus bzw. letzter Klick in seinem Fenster), nimmt sie.
        for (open, salt, win) in [
            (self.new_dialog.is_some(), "newdlg", "new_session_dialog"),
            (self.session_settings.is_some(), "sset", "session_settings"),
            (self.tpl.form.is_some(), "tpled", "tpl_editor"),
            (self.settings_open && self.settings_tab == 2, "defaults", "settings_win"),
        ] {
            if open {
                opts_tabs::grab_keys(ctx, salt, Id::new(win));
            }
        }
        self.titlebar(ctx);
        self.backup_tick(ctx);
        self.window_edges(ctx);
        self.mode_shortcuts(ctx);
        if self.mode == titlebar::Mode::Terminals {
            self.shortcuts(ctx, &mut actions);
            self.sidebar(ctx, &mut actions);
            self.files_ui(ctx, &mut actions);
            self.central(ctx, &mut actions);
            self.handle_drag(ctx);
        } else if self.mode == titlebar::Mode::Editor {
            self.editor_view(ctx);
        } else if self.mode == titlebar::Mode::Time {
            self.time_view(ctx);
        } else {
            self.tasks_view(ctx);
        }
        if self.main_overlay() {
            self.notice_ui(ctx);
            self.toasts_ui(ctx);
            self.access_dialog_ui(ctx);
            self.time_overlay(ctx);
        }
        self.detached_windows(ctx, &mut actions);
        self.text_prompt_ui(ctx, &mut actions);
        self.dialogs(ctx, &mut actions);
        if self.picker_here() {
            self.tlinks_picker_ui(ctx);
        }
        self.restore_dialog(ctx);
        self.cleanup_ui(ctx);
        self.update_frame(ctx);
        // Tutorial (#143) zuletzt: liest die in diesem Bild gemeldeten Ziele und zeichnet über allem.
        self.tut_frame(ctx, &mut actions);
        for a in actions {
            self.apply(ctx, a);
        }
        self.focus_return_tick(ctx);

        // Fenstertitel
        let waiting = self.entries.values().filter(|e| e.needs_you()).count();
        let prefix = if waiting > 0 { format!("({waiting}) ") } else { String::new() };
        let title = match self.entries.get(&self.focused) {
            Some(e) => format!("{prefix}{} – Paneldeck", e.display_name()),
            None => format!("{prefix}Paneldeck"),
        };
        if title != self.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }
        // Weitere Fenster (Strg+Klick auf einen Reiter) nach dem Hauptfenster zeichnen.
        self.hub_windows(ctx);

        if self.last_save.elapsed() > Duration::from_secs(60) {
            self.save_all();
            self.last_save = Instant::now();
        }
        let animating = self.entries.values().any(|e| e.status == ClaudeStatus::Working && !e.exited());
        ctx.request_repaint_after(Duration::from_millis(if animating { 80 } else { 500 }));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        crate::platform::chrome::shutdown();
        // Reiter „Zeit“ (#164): ein klingelnder Wecker verstummt mit dem Programm.
        crate::platform::sound::stop_all();
        self.save_for_exit();
        // Editor (#54) unabhängig von „Sitzungen merken“: Reiter und ungespeicherte Puffer (Hot Exit) immer sichern.
        self.ed.save(&self.dir);
        // Hilfsdateien der zsh/fish-Integration (#57) entfernen.
        crate::shellinit::remove_files(&self.dir);
    }
}

// -------------------------------------------------------------- Bereiche

struct PaneCx<'a> {
    entries: &'a mut BTreeMap<u64, Entry>,
    theme: &'a Theme,
    font: &'a FontId,
    focused: u64,
    want_focus: &'a mut Option<u64>,
    actions: &'a mut Vec<Action>,
    lang: Lang,
    claude_mode: bool,
    rects: &'a mut HashMap<u64, Rect>,
    broadcast: bool,
    usage: &'a UsageWatcher,
    git: &'a GitWatcher,
    dragging: &'a mut Option<u64>,
    detached: bool,
    themes: &'a [Theme],
    profiles: &'a [Profile],
    /// Verknüpfte Tickets je Sitzung (Chips im Kopf).
    tl: &'a terminal_links::TLinks,
    /// Einstellung „Mausrad in Vollbildprogrammen“.
    alt_wheel: state::AltScreenWheel,
    /// Kürzel „Bereich minimieren“ für den Tooltip (eigene Belegung, Sprache).
    hide_key: &'a str,
    /// Hintergrund eines Chats (Deckkraft des Bereichs „Chat“, #387).
    chat_bg: Color32,
}


const GAP: f32 = 6.0;
const HEADER_H: f32 = 30.0;
/// Kleinste Breite/Höhe eines Bereichs, solange der Platz reicht (Kopfzeile und ein paar Zeilen/Spalten bleiben sichtbar).
const MIN_PANE_W: f32 = 120.0;
const MIN_PANE_H: f32 = HEADER_H + 40.0;

/// Kleinste Ausdehnung eines Teilbaums in Richtung `dir` (Breite bei `Horizontal`, Höhe bei `Vertical`): ein Bereich hat
/// `MIN_PANE_W`/`MIN_PANE_H`; eine Teilung in derselben Richtung braucht beide Seiten plus Trennlinie, eine quer dazu die
/// größere der beiden Seiten.
fn min_extent(node: &Node, dir: Dir) -> f32 {
    match node {
        Node::Leaf(_) => match dir {
            Dir::Horizontal => MIN_PANE_W,
            Dir::Vertical => MIN_PANE_H,
        },
        Node::Split { dir: d, a, b, .. } if *d == dir => min_extent(a, dir) + GAP + min_extent(b, dir),
        Node::Split { a, b, .. } => min_extent(a, dir).max(min_extent(b, dir)),
    }
}

/// Kern von `App::activate`: Sitzung `id` nach `layout::show` ins Layout bringen. Eine minimierte öffnet sich allein; die
/// Bereiche, die dabei das Layout verlassen, werden als minimiert markiert. Den veralteten `slot` (2.2.0) beider Seiten löschen.
/// Liefert, wie viele Bereiche dabei minimiert wurden.
fn show_session(layout: &mut Option<Node>, entries: &mut BTreeMap<u64, Entry>, id: u64, focused: u64) -> usize {
    let Some(e) = entries.get_mut(&id) else { return 0 };
    let minimized = std::mem::take(&mut e.meta.minimized);
    e.meta.slot = None;
    mark_minimized(entries, &crate::layout::show(layout, id, focused, minimized))
}

/// Kern von `App::place` und Strg+Tab: Sitzung `id` nach `layout::place` ins Layout bringen (sie ist danach nicht mehr
/// minimiert); die verdrängten Bereiche (`Solo`, `Replace`) werden als minimiert markiert. Liefert, wie viele das sind.
fn place_session(layout: &mut Option<Node>, entries: &mut BTreeMap<u64, Entry>, id: u64, anchor: u64, placement: crate::layout::Placement, protected: &dyn Fn(u64) -> bool) -> usize {
    let Some(e) = entries.get_mut(&id) else { return 0 };
    e.meta.minimized = false;
    e.meta.slot = None;
    mark_minimized(entries, &crate::layout::place(layout, id, anchor, placement, protected).minimized)
}

/// Fokus nach einer Platzierung für einen Agenten: bleibt `focused`, solange es im Layout steht; sonst (ersetzt, minimiert,
/// leerer Hub) die gezeigte Sitzung `shown`.
fn keep_focus(layout: Option<&Node>, focused: u64, shown: u64) -> u64 {
    if layout.is_some_and(|l| l.contains(focused)) { focused } else { shown }
}

fn mark_minimized(entries: &mut BTreeMap<u64, Entry>, ids: &[u64]) -> usize {
    for gone in ids {
        if let Some(e) = entries.get_mut(gone) {
            e.meta.minimized = true;
            e.meta.slot = None;
        }
    }
    ids.len()
}

/// Aktiver Bereich für einen Agenten (#162): der Bereich seiner eigenen Sitzung, wenn sie im gezeichneten Layout steht,
/// sonst der Bereich, den der Nutzer fokussiert hat.
fn agent_anchor(layout: Option<&Node>, caller: Option<u64>, focused: u64) -> u64 {
    caller.filter(|c| layout.is_some_and(|l| l.contains(*c))).unwrap_or(focused)
}

/// Nächste Sitzung für Strg+Tab (`delta` = +1/−1) in `ids` ab `focused` (nicht dabei: die erste); `None` ohne Sitzungen.
fn cycle_target(ids: &[u64], focused: u64, delta: i32) -> Option<u64> {
    if ids.is_empty() {
        return None;
    }
    let Some(pos) = ids.iter().position(|i| *i == focused) else { return Some(ids[0]) };
    Some(ids[(pos as i32 + delta).rem_euclid(ids.len() as i32) as usize])
}

/// Kurzer Hinweis, nachdem eine Sitzung allein geöffnet wurde (#162).
fn minimized_hint(n: usize, lang: Lang) -> String {
    if n == 1 {
        lang.t("1 Bereich minimiert", "1 pane minimized").to_string()
    } else {
        format!("{n} {}", lang.t("Bereiche minimiert", "panes minimized"))
    }
}

/// Was im Layout steht, ist nicht minimiert – egal, wie es hineinkam (Ziehen, Ordner-Layout, Vorlage …). Läuft in jedem Bild
/// (billig: nur die Blätter des gezeichneten Layouts).
fn unminimize_shown(layout: Option<&Node>, entries: &mut BTreeMap<u64, Entry>) {
    let Some(l) = layout else { return };
    for id in l.leaf_ids() {
        if let Some(e) = entries.get_mut(&id).filter(|e| e.meta.minimized || e.meta.slot.is_some()) {
            e.meta.minimized = false;
            e.meta.slot = None;
        }
    }
}

/// Pixel der ersten Seite einer Teilung der Länge `len` (ohne Trennlinie): Anteil `ratio` (begrenzt, NaN → Mitte), dabei
/// Seite a mindestens `min_a`, Seite b mindestens `min_b`, solange `len` dafür reicht – sonst einfach nach Anteil.
fn split_px(len: f32, ratio: f32, min_a: f32, min_b: f32) -> f32 {
    let len = len.max(0.0);
    let px = (len * crate::layout::clamp_ratio(ratio)).round();
    if len >= min_a + min_b { px.clamp(min_a, len - min_b) } else { px }
}

/// Neuer Anteil nach dem Ziehen der Trennlinie um `delta` Pixel: begrenzt auf `RATIO_MIN..RATIO_MAX` und so, dass Seite a
/// nicht unter `min_a`, Seite b nicht unter `min_b` Pixel fällt (soweit der Platz reicht). Bei einer Länge ≤ 0 (Fenster
/// minimiert) bleibt er.
fn drag_ratio(ratio: f32, delta: f32, len: f32, min_a: f32, min_b: f32) -> f32 {
    if len <= 1.0 || !delta.is_finite() {
        return crate::layout::clamp_ratio(ratio);
    }
    let r = crate::layout::clamp_ratio(ratio + delta / len);
    if len >= min_a + min_b { r.clamp(min_a / len, 1.0 - min_b / len) } else { r }
}

fn mix_col(a: Color32, b: Color32, t: f32) -> Color32 {
    style::mix(a, b, t)
}

fn show_node(ui: &mut Ui, node: &mut Node, rect: Rect, cx: &mut PaneCx) {
    match node {
        Node::Leaf(id) => show_pane(ui, *id, rect, cx),
        Node::Split { dir, ratio, a, b } => {
            // Mindestgröße je Teilbaum (eine geschachtelte Seite braucht Platz für alle ihre Bereiche).
            let (min_a, min_b) = (min_extent(a, *dir), min_extent(b, *dir));
            let (ra, rsep, rb) = match dir {
                Dir::Horizontal => {
                    let w = rect.width() - GAP;
                    let wa = split_px(w, *ratio, min_a, min_b);
                    (
                        Rect::from_min_size(rect.min, Vec2::new(wa, rect.height())),
                        Rect::from_min_size(Pos2::new(rect.min.x + wa, rect.min.y), Vec2::new(GAP, rect.height())),
                        Rect::from_min_max(Pos2::new(rect.min.x + wa + GAP, rect.min.y), rect.max),
                    )
                }
                Dir::Vertical => {
                    let h = rect.height() - GAP;
                    let ha = split_px(h, *ratio, min_a, min_b);
                    (
                        Rect::from_min_size(rect.min, Vec2::new(rect.width(), ha)),
                        Rect::from_min_size(Pos2::new(rect.min.x, rect.min.y + ha), Vec2::new(rect.width(), GAP)),
                        Rect::from_min_max(Pos2::new(rect.min.x, rect.min.y + ha + GAP), rect.max),
                    )
                }
            };
            let (mut la, mut lb) = (Vec::new(), Vec::new());
            a.leaves(&mut la);
            b.leaves(&mut lb);
            let sep_id = Id::new(("sep", la[0], lb[0]));
            let resp = ui.interact(rsep, sep_id, Sense::drag());
            if resp.hovered() || resp.dragged() {
                ui.ctx().set_cursor_icon(match dir {
                    Dir::Horizontal => egui::CursorIcon::ResizeHorizontal,
                    Dir::Vertical => egui::CursorIcon::ResizeVertical,
                });
            }
            if resp.dragged() {
                let d = resp.drag_delta();
                // Von der gezeichneten Lage aus ziehen (die kann wegen der Mindestgröße vom gespeicherten Anteil abweichen).
                let (delta, len, cur) = match dir {
                    Dir::Horizontal => (d.x, rect.width() - GAP, ra.width()),
                    Dir::Vertical => (d.y, rect.height() - GAP, ra.height()),
                };
                *ratio = drag_ratio(cur / len.max(1.0), delta, len, min_a, min_b);
            }
            let hv = style::anim(ui.ctx(), sep_id.with("h"), resp.hovered() || resp.dragged(), style::D_FAST);
            ui.painter().rect_filled(rsep, 0.0, ui.visuals().panel_fill);
            let line = match dir {
                Dir::Horizontal => Rect::from_center_size(rsep.center(), Vec2::new(1.0 + 2.0 * hv, rsep.height())),
                Dir::Vertical => Rect::from_center_size(rsep.center(), Vec2::new(rsep.width(), 1.0 + 2.0 * hv)),
            };
            let base = ui.visuals().widgets.noninteractive.bg_stroke.color;
            ui.painter().rect_filled(line, 0.0, style::mix(base, style::brand(), hv));
            show_node(ui, a, ra, cx);
            show_node(ui, b, rb, cx);
        }
    }
}

/// Knöpfe im Kopf eines Bereichs, von rechts nach links (wichtigste zuerst).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PaneBtn {
    Close,
    Minimize,
    Reload,
    Detach,
    SplitDown,
    SplitRight,
    Link,
}

const PANE_BTN_W: f32 = 22.0;

/// Alle Knöpfe eines Bereichs (abgedockt: Andocken, Minimieren, ggf. Neu laden).
fn pane_buttons(detached: bool, reload: bool, link: bool) -> Vec<PaneBtn> {
    let mut v = vec![PaneBtn::Close, PaneBtn::Minimize];
    if reload {
        v.push(PaneBtn::Reload);
    }
    if !detached {
        v.extend([PaneBtn::Detach, PaneBtn::SplitDown, PaneBtn::SplitRight]);
        if link {
            v.push(PaneBtn::Link);
        }
    }
    v
}

/// Wie viele der `n` Knöpfe in einen Kopf der Breite `width` passen: links bleiben Statuspunkt und ein Stück Name
/// (60 Punkte) frei, rechts etwas Luft; Schließen und Minimieren stehen immer da (#259).
fn pane_buttons_fit(width: f32, n: usize) -> usize {
    let room = width - 16.0 - 60.0;
    ((room / PANE_BTN_W).floor().max(0.0) as usize).clamp(2.min(n), n)
}

fn show_pane(ui: &mut Ui, id: u64, rect: Rect, cx: &mut PaneCx) {
    let lang = cx.lang;
    cx.rects.insert(id, rect);
    let focused = cx.focused == id;
    let header = Rect::from_min_size(rect.min, Vec2::new(rect.width(), HEADER_H));
    let body = Rect::from_min_max(Pos2::new(rect.min.x, rect.min.y + HEADER_H), rect.max);

    let Some(entry) = cx.entries.get_mut(&id) else {
        ui.painter().rect_filled(rect, 0.0, cx.theme.bg);
        return;
    };
    let accent = entry.color();
    let name = entry.display_name();
    let tl_enabled = cx.tl.enabled && !cx.detached;

    // Kopfzeile
    let t = ui.input(|i| i.time);
    let hbg = ui.visuals().faint_bg_color;
    ui.painter().rect_filled(header, CornerRadius::ZERO, hbg);
    let fo = style::anim(ui.ctx(), Id::new(("hdr_focus", id)), focused, style::D_BASE);
    if fo > 0.0 {
        ui.painter().rect_filled(Rect::from_min_size(header.min, Vec2::new(header.width(), 2.0 * fo)), 0.0, accent);
    }
    ui.painter().line_segment(
        [Pos2::new(header.min.x, header.max.y - 0.5), Pos2::new(header.max.x, header.max.y - 0.5)],
        Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color.gamma_multiply(0.7)),
    );
    // Kopfzeile = Titelleiste des Bereichs (Klick fokussiert, Ziehen verschiebt): kein Knopf, darum ohne Hand (#187).
    let hresp = ui.interact(header, Id::new(("hdr", id)), if cx.detached { Sense::click() } else { Sense::click_and_drag() }).no_pointer();
    if hresp.drag_started() {
        *cx.dragging = Some(id);
    }
    if hresp.clicked() {
        cx.actions.push(Action::Focus(id));
    }
    // „Neu laden“ (#169) nur für Terminals; ein Chat hat keinen Prozess.
    let reload = session_settings::reload_plan(&entry.meta);
    // Knöpfe rechts im Kopf (von rechts nach links). In schmalen Bereichen nur so viele, wie hineinpassen – sonst ragten sie
    // in den Kopf des Nachbarn, und ein Klick träfe dessen Teilen/Schließen (#259); die übrigen stehen im Kontextmenü.
    let all_btns = pane_buttons(cx.detached, reload.is_some(), tl_enabled);
    let fit = if cx.detached { all_btns.len() } else { pane_buttons_fit(header.width(), all_btns.len()) };
    let (shown_btns, hidden_btns) = all_btns.split_at(fit);
    hresp.context_menu(|ui| {
        if ui.button(lang.t("Bereich minimieren", "Minimize pane")).clicked() {
            cx.actions.push(Action::MinimizePane(id));
            ui.close();
        }
        if tl_enabled && ui.button(lang.t("Ticket verknüpfen …", "Link ticket …")).clicked() {
            cx.actions.push(Action::LinkTicket(id));
            ui.close();
        }
        for b in hidden_btns {
            let (label, action) = match b {
                PaneBtn::Reload => (lang.t("Neu laden …", "Reload …"), Action::RequestReload(id)),
                PaneBtn::Detach => (lang.t("In eigenem Fenster öffnen", "Open in own window"), Action::Detach(id)),
                PaneBtn::SplitDown => (lang.t("Darunter teilen", "Split below"), Action::Split(id, Dir::Vertical)),
                PaneBtn::SplitRight => (lang.t("Rechts teilen", "Split right"), Action::Split(id, Dir::Horizontal)),
                _ => continue, // Schließen/Minimieren stehen immer da, Verknüpfen schon oben
            };
            if ui.button(label).clicked() {
                cx.actions.push(action);
                ui.close();
            }
        }
    });
    let claude = entry.is_claude() && cx.claude_mode;
    let status_label = entry.status_label(lang);
    let status_col = entry.status_color(t);
    let (text_c, strong_c, weak) = {
        let v = ui.visuals();
        (v.text_color(), v.strong_text_color(), v.weak_text_color())
    };
    // links: Statuspunkt, (Claude-Marke), Name
    let cy = header.center().y + 1.0;
    ui.painter().circle_filled(Pos2::new(header.min.x + 14.0, cy), 3.6, if entry.exited() { Color32::from_gray(110) } else { status_col });
    let mut hx = header.min.x + 26.0;
    if claude {
        style::paint_icon(ui.painter(), SIcon::SparkSmall, Pos2::new(hx + 4.0, cy), style::brand());
        hx += 14.0;
    }
    let name_col = mix_col(text_c, strong_c, fo);
    // Breite des Knopfbereichs rechts: die gezeigten Knöpfe (je 22) und etwas Luft.
    let btns_w = PANE_BTN_W * shown_btns.len() as f32 + 16.0;
    let right_edge = header.max.x - (btns_w + 4.0);
    let avail = right_edge - hx - 8.0;
    let chips = cx.tl.chips_of(id);
    let chip_est = if chips.is_empty() { 0.0 } else { terminal_links::chip_label(&chips.iter().map(|c| c.key.as_str()).collect::<Vec<_>>()).chars().count() as f32 * 7.0 + 46.0 };
    let pill_est = chip_est + if status_label.is_empty() { 0.0 } else { status_label.chars().count() as f32 * 7.0 + 26.0 };
    // Der Name hat Vorrang vor Zusatzinfos; die Status-Pille bekommt Platz, solange der Name noch ~90 px behält.
    let name_w = if avail - pill_est >= 90.0 { avail - pill_est } else { avail }.max(30.0);
    let mut job = egui::text::LayoutJob::simple_singleline(name, FontId::proportional(13.0), name_col);
    job.wrap = egui::text::TextWrapping { max_width: name_w, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let name_galley = ui.painter().layout_job(job);
    let name_end = hx + name_galley.size().x;
    ui.painter().with_clip_rect(header).galley(Pos2::new(hx, cy - name_galley.size().y / 2.0), name_galley, name_col);

    // rechts (von rechts nach links, nur wenn Platz ist): Status-Pille, Token/Kosten, Git
    let mut x = right_edge;
    if !chips.is_empty() {
        let w = terminal_links::chip_row(ui, Id::new(("tlchip", id)), id, chips, Pos2::new(x, cy), x - name_end - 16.0, lang, cx.actions);
        if w > 0.0 {
            x -= w + 10.0;
        }
    }
    if !status_label.is_empty() && x - name_end > pill_est * 0.8 {
        let w = style::status_pill(ui.painter(), Pos2::new(x, cy), status_label, status_col);
        x -= w + 10.0;
    }
    let mut infos: Vec<String> = Vec::new();
    if entry.is_chat() {
        // Chat: Modellname statt Token/Git.
        infos.push(entry.chat_model().to_string());
    }
    if !entry.meta.queue.is_empty() {
        infos.push(format!("Queue {}", entry.meta.queue.len()));
    }
    if cx.claude_mode {
        if let Some(u) = cx.usage.get(id) {
            infos.push(usage_line(&u, lang));
        }
    }
    if let Some(g) = cx.git.get(id).filter(|_| !entry.is_chat()) {
        infos.push(git_line(&g));
    }
    // Umgebungsvariablen (#384): `$ n`, Tooltip mit den Namen, oranger Punkt bis zum Neustart.
    if !entry.is_chat() && x - name_end > 40.0 {
        let w = env_ui::header_badge(ui, id, &entry.meta.env, entry.env_pending, Pos2::new(x, cy), lang);
        if w > 0.0 {
            x -= w + 10.0;
        }
    }
    for text in infos {
        let g = ui.painter().layout_no_wrap(text, FontId::proportional(11.5), weak);
        let w = g.size().x;
        if x - w < name_end + 16.0 {
            break;
        }
        ui.painter().galley(Pos2::new(x - w, cy - g.size().y / 2.0), g, weak);
        x -= w + 12.0;
    }
    ui.scope_builder(
        UiBuilder::new().max_rect(Rect::from_min_max(
            Pos2::new(header.max.x - btns_w, header.min.y + 2.0),
            Pos2::new(header.max.x - 4.0, header.max.y),
        )),
        |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                for b in shown_btns {
                    let (icon, tip, action): (SIcon, String, Action) = match b {
                        PaneBtn::Close if cx.detached => (SIcon::Close, lang.t("Wieder andocken (Strg+Umschalt+W)", "Dock back (Ctrl+Shift+W)").into(), Action::Attach(id)),
                        PaneBtn::Close => (SIcon::Close, lang.t("Sitzung schließen", "Close session").into(), Action::RequestClose(id)),
                        PaneBtn::Minimize if cx.detached => (
                            SIcon::Collapse,
                            lang.t("Minimieren – Fenster schließen, Sitzung läuft weiter (in der Seitenleiste zurückholen)", "Minimize – close the window, the session keeps running (bring it back from the sidebar)").into(),
                            Action::MinimizePane(id),
                        ),
                        PaneBtn::Minimize => {
                            let tip = lang.t("Bereich minimieren – die Sitzung läuft weiter, ein Klick in der Seitenleiste holt sie zurück", "Minimize pane – the session keeps running, click it in the sidebar to bring it back");
                            (SIcon::Collapse, if cx.hide_key.is_empty() { tip.to_string() } else { format!("{tip} ({})", cx.hide_key) }, Action::MinimizePane(id))
                        }
                        PaneBtn::Reload => match &reload {
                            Some(plan) => (SIcon::Reload, plan.tip(lang).to_string(), Action::RequestReload(id)),
                            None => continue,
                        },
                        PaneBtn::Detach => (SIcon::Detach, lang.t("In eigenem Fenster öffnen", "Open in own window").into(), Action::Detach(id)),
                        PaneBtn::SplitDown => (SIcon::SplitDown, lang.t("Darunter teilen (Strg+Umschalt+E)", "Split below (Ctrl+Shift+E)").into(), Action::Split(id, Dir::Vertical)),
                        PaneBtn::SplitRight => (SIcon::SplitRight, lang.t("Rechts teilen (Strg+Umschalt+D)", "Split right (Ctrl+Shift+D)").into(), Action::Split(id, Dir::Horizontal)),
                        PaneBtn::Link => (SIcon::Link, lang.t("Ticket verknüpfen …", "Link ticket …").into(), Action::LinkTicket(id)),
                    };
                    let r = style::icon_btn_small(ui, icon, &tip);
                    // Tutorial (#143): Teilen/Minimieren des fokussierten Bereichs.
                    if id == cx.focused && !cx.detached {
                        match b {
                            PaneBtn::SplitRight => tutorial::spot_rect(ui, Spot::PaneSplit, r.rect),
                            PaneBtn::Minimize => tutorial::spot_rect(ui, Spot::PaneMinimize, r.rect),
                            _ => {}
                        }
                    }
                    if r.clicked() {
                        cx.actions.push(action);
                    }
                }
            });
        },
    );

    // Chat (keine PTY): Nachrichten und Eingabe; Senden usw. führt die App aus.
    if let Some(chat) = entry.chat.as_mut() {
        ui.painter().rect_filled(body, 0.0, cx.chat_bg);
        let grab = *cx.want_focus == Some(id);
        if grab {
            *cx.want_focus = None;
        }
        let model = entry.meta.chat.as_ref().map(|c| c.model.clone()).unwrap_or_default();
        let o = chat_ui::chat_body(ui, body, id, chat, &model, lang, grab);
        if o.focus && cx.focused != id {
            cx.actions.push(Action::Focus(id));
        }
        for (hit, a) in [(o.send, Action::ChatSend(id)), (o.stop, Action::ChatStop(id)), (o.regenerate, Action::ChatRegenerate(id)), (o.edit, Action::ChatEdit(id)), (o.cont, Action::ChatContinue(id)), (o.compact, Action::ChatCompact(id))] {
            if hit {
                cx.actions.push(a);
            }
        }
        if let Some((call, d)) = o.decide {
            cx.actions.push(Action::ChatDecide(id, call, d));
        }
        if let Some(rule) = o.revoke {
            cx.actions.push(Action::ChatRevoke(id, rule));
        }
        if let Some(level) = o.level {
            cx.actions.push(Action::ChatLevel(id, level));
        }
        if let Some(act) = o.plan {
            cx.actions.push(Action::ChatPlan(id, act));
        }
        if let Some(req) = o.image {
            cx.actions.push(Action::ChatImage(id, req));
        }
        if !cx.detached {
            edge_buttons(ui, id, body, cx, false);
        }
        // Kein Broadcast-Rahmen: Chats bekommen keine Broadcast-Eingaben.
        return;
    }

    // Terminal
    let cwd_now = entry.cwd_now();
    let profile_id = entry.meta.profile;
    match entry.live.as_mut() {
        Some(session) => {
            let tid = Id::new(("term", id));
            // Erst NACH `termview::show` fokussieren: egui nimmt einem fokussierten Widget den Fokus beim `interact`
            // wieder weg, wenn im selben Bild woanders geklickt wurde (Reiter, Palette, Dialogknopf; #238).
            let grab = *cx.want_focus == Some(id);
            if grab {
                *cx.want_focus = None;
            }
            let prof = cx.profiles.iter().find(|p| p.id == profile_id);
            let mut theme_local = None;
            if let Some(t) = prof.filter(|p| !p.theme.is_empty()).and_then(|p| cx.themes.iter().find(|t| t.name == p.theme)) {
                let mut t = t.clone();
                t.bg = Color32::from_rgba_unmultiplied(t.bg.r(), t.bg.g(), t.bg.b(), cx.theme.bg.a());
                theme_local = Some(t);
            }
            let font_local = prof.filter(|p| p.font_size > 0.0).map(|p| FontId::monospace(p.font_size));
            let resp = termview::show(ui, body, tid, session, theme_local.as_ref().unwrap_or(cx.theme), font_local.as_ref().unwrap_or(cx.font), &cwd_now, cx.alt_wheel);
            if grab {
                ui.memory_mut(|m| m.request_focus(tid));
                ui.ctx().request_repaint();
            }
            if let Some((text, check_targets)) = session.take_paste_ask() {
                cx.actions.push(Action::PasteAsk(id, text, cx.detached, check_targets));
            }
            if session.search.is_some() && search_bar(ui, session, body, lang) {
                session.search = None;
                *cx.want_focus = Some(id);
            }
            if resp.clicked() || resp.drag_started() || resp.secondary_clicked() {
                cx.actions.push(Action::Focus(id));
            } else if resp.has_focus() && cx.focused != id {
                cx.actions.push(Action::Focus(id));
            }
            if session.has_exited() {
                let bar = Rect::from_min_max(Pos2::new(body.min.x, body.max.y - 30.0), body.max);
                ui.painter().rect_filled(bar, 0.0, Color32::from_black_alpha(190));
                ui.scope_builder(UiBuilder::new().max_rect(bar.shrink2(Vec2::new(8.0, 3.0))), |ui| {
                    ui.horizontal(|ui| {
                        ui.colored_label(Color32::from_gray(220), lang.t("Prozess beendet.", "Process exited."));
                        // Wie „Neu laden“: mit Hintergrundprozessen im Job erst fragen (#213).
                        if ui.button(lang.t("Neu starten", "Restart")).clicked() {
                            cx.actions.push(Action::RequestReload(id));
                        }
                    });
                });
            }
        }
        None => {
            ui.painter().rect_filled(body, 0.0, cx.theme.bg);
            let msg = entry.error.clone().unwrap_or_else(|| "—".into());
            ui.scope_builder(UiBuilder::new().max_rect(body.shrink(12.0)), |ui| {
                if entry.pick_pending {
                    // Kein Fehler: die Sitzung startet nach der Auswahl der Konversation (#302). Die Auswahl erscheint im
                    // Hauptfenster; von hier aus lässt sie sich dorthin holen oder überspringen.
                    ui.label(msg);
                    ui.horizontal(|ui| {
                        if ui.button(lang.t("Auswahl zeigen", "Show the choice")).clicked() {
                            cx.actions.push(Action::ConvPickShow);
                        }
                        if ui.button(lang.t("Ohne Auswahl neu starten", "Start new without choosing")).clicked() {
                            cx.actions.push(Action::ConvPickFresh(id));
                        }
                    });
                    return;
                }
                if let Some(l) = &entry.env_locked {
                    // Geheime Variablen gesperrt (#384): kein Fehler, die Sitzung wartet wie eine Administrator-Sitzung.
                    ui.label(RichText::new(&msg).color(crate::style::ui().warning));
                    ui.horizontal(|ui| {
                        if l.needs_reentry() {
                            if ui.button(lang.t("Variablen bearbeiten …", "Edit variables …")).clicked() {
                                cx.actions.push(Action::EnvEdit(id));
                            }
                        } else if ui.button(lang.t("Entsperren und starten", "Unlock and start")).clicked() {
                            cx.actions.push(Action::EnvUnlock(id));
                        }
                    });
                    return;
                }
                ui.colored_label(Color32::LIGHT_RED, format!("{}: {msg}", lang.t("Start fehlgeschlagen", "Failed to start")));
                if ui.button(lang.t("Neu starten", "Restart")).clicked() {
                    cx.actions.push(Action::RequestReload(id));
                }
            });
        }
    }

    if !cx.detached {
        edge_buttons(ui, id, body, cx, true);
    }
    if cx.broadcast {
        ui.painter().rect_stroke(rect, 0.0, Stroke::new(1.5_f32, crate::style::brand()), StrokeKind::Inside);
    }
}

/// Suchleiste oben rechts im Bereich. Gibt `true` zurück, wenn sie geschlossen werden soll.
fn search_bar(ui: &mut Ui, session: &mut Session, body: Rect, lang: Lang) -> bool {
    let Some(mut st) = session.search.take() else { return false };
    let mut close = false;
    let bar = Rect::from_min_size(Pos2::new(body.max.x - 360.0 - 14.0, body.min.y + 6.0), Vec2::new(360.0, 28.0));
    ui.painter().rect_filled(bar, 6.0, ui.visuals().window_fill());
    ui.painter().rect_stroke(bar, 6.0, Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color), StrokeKind::Inside);
    ui.scope_builder(UiBuilder::new().max_rect(bar.shrink2(Vec2::new(6.0, 3.0))), |ui| {
        ui.horizontal(|ui| {
            let te = ui.add(
                egui::TextEdit::singleline(&mut st.query)
                    .desired_width(180.0)
                    .hint_text(lang.t("Im Verlauf suchen …", "Search scrollback …")),
            );
            if st.focus_input {
                te.request_focus();
                st.focus_input = false;
            }
            let (enter, shift, esc) = ui.input(|i| (i.key_pressed(Key::Enter), i.modifiers.shift, i.key_pressed(Key::Escape)));
            let mut step = 0i32;
            if te.lost_focus() && enter {
                step = if shift { -1 } else { 1 };
                te.request_focus();
            }
            if (te.has_focus() || te.lost_focus()) && esc {
                close = true;
            }
            let (prev, next) = (ui.small_button("<").clicked(), ui.small_button(">").clicked());
            if prev {
                step = -1;
            }
            if next {
                step = 1;
            }
            let changed = st.query != st.last_query;
            let now = ui.input(|i| i.time);
            let activity = session.output_stamp();
            if changed || step != 0 {
                let mut t = session.term.lock().unwrap_or_else(|e| e.into_inner());
                let base = session.hist_base();
                if changed {
                    st.compute(&t, base, activity);
                    st.current = st.matches.len().saturating_sub(1);
                } else {
                    // erst auf den aktuellen Stand bringen (derselbe Treffer), dann weiter (#250)
                    st.refresh(&t, base, activity);
                    let n = st.matches.len();
                    st.current = if n == 0 { 0 } else { (st.current as i32 + step).rem_euclid(n as i32) as usize };
                }
                st.refreshed = now;
                st.last_query = st.query.clone();
                if let Some(m) = st.matches.get(st.current) {
                    search::scroll_to(&mut t, m.line);
                }
            } else {
                // Neue Ausgabe: Treffer nachführen, damit Markierungen nicht wandern und `current` gültig bleibt (#250);
                // bei laufender Ausgabe höchstens viermal pro Sekunde.
                let t = session.term.lock().unwrap_or_else(|e| e.into_inner());
                let base = session.hist_base();
                if st.stale(&t, base, activity) {
                    if now - st.refreshed >= 0.25 {
                        st.refresh(&t, base, activity);
                        st.refreshed = now;
                    } else {
                        ui.ctx().request_repaint_after(Duration::from_millis(260));
                    }
                }
            }
            let n = st.matches.len();
            ui.weak(if n == 0 { "0".to_string() } else { format!("{}/{}", st.current + 1, n) });
            if style::icon_btn_small(ui, SIcon::Close, lang.t("Schließen (Esc)", "Close (Esc)")).clicked() {
                close = true;
            }
        });
    });
    if !close {
        session.search = Some(st);
    }
    close
}

fn usage_line(u: &Usage, lang: Lang) -> String {
    format!(
        "ctx {} · {} in · {} out · ≈ ${:.2}{}",
        fmt_tokens(u.context),
        fmt_tokens(u.total.input + u.total.cache_read + u.total.cache_write),
        fmt_tokens(u.total.output),
        u.cost(),
        if lang == Lang::De { "" } else { "" }
    )
}

fn git_line(g: &GitInfo) -> String {
    if g.changed > 0 {
        format!("git: {} +{}", g.branch, g.changed)
    } else {
        format!("git: {}", g.branch)
    }
}

use std::path::Path;

#[derive(Clone)]
struct TreeItem {
    name: String,
    path: PathBuf,
    is_dir: bool,
}

fn list_dir(p: &Path, hidden: bool) -> Vec<TreeItem> {
    let mut v: Vec<TreeItem> = std::fs::read_dir(p)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    if !hidden && name.starts_with('.') {
                        return None;
                    }
                    let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                    Some(TreeItem { name, path: e.path(), is_dir })
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    v.truncate(500);
    v
}

/// Ein Ordner des Dateibaums mit Inhalt; `mark` = aktueller Ordner der Shell (aufklappen und markieren, #277).
fn tree_children(ui: &mut Ui, dir: &Path, hidden: bool, cache: &mut HashMap<PathBuf, Vec<TreeItem>>, actions: &mut Vec<Action>, lang: Lang, mark: &panels::TreeMark) {
    // Langsame Wurzel (Netzwerk, WSL): im Hilfsthread lesen, bis dahin „wird geladen …“ (#277 Review).
    let items = if mark.slow {
        match panels::slow_list(ui.ctx(), dir, hidden) {
            Some(v) => v,
            None => {
                ui.weak(lang.t("wird geladen …", "loading …"));
                return;
            }
        }
    } else {
        cache.entry(dir.to_path_buf()).or_insert_with(|| list_dir(dir, hidden)).clone()
    };
    if items.is_empty() {
        ui.weak(lang.t("(leer)", "(empty)"));
    }
    for it in items {
        let path_s = it.path.to_string_lossy().to_string();
        let menu = |ui: &mut Ui, actions: &mut Vec<Action>| {
            if ui.button(lang.t("Pfad einfügen", "Insert path")).clicked() {
                actions.push(Action::InsertPath(path_s.clone()));
                ui.close();
            }
            if it.is_dir && ui.button(lang.t("Neue Sitzung hier", "New session here")).clicked() {
                actions.push(Action::NewSessionAt(path_s.clone()));
                ui.close();
            }
            if crate::platform::open::FILE_MANAGER && ui.button(if cfg!(windows) { lang.t("Im Explorer anzeigen", "Show in Explorer") } else { lang.t("Im Dateimanager anzeigen", "Show in file manager") }).clicked() {
                crate::platform::open::show_in_file_manager(&path_s, it.is_dir);
                ui.close();
            }
        };
        if it.is_dir {
            let (on_way, current) = mark.role(&it.path);
            let mut name = RichText::new(&it.name).strong();
            if current {
                name = name.color(style::accent());
            }
            // Auf dem Weg zum aktuellen Ordner der Shell: einmal aufklappen, wenn er sich ändert (danach frei zuklappbar).
            let r = egui::CollapsingHeader::new(name).id_salt(("tree", &it.path)).open((on_way && mark.reveal && (!mark.slow || panels::slow_cached(&it.path, hidden))).then_some(true)).show(ui, |ui| {
                tree_children(ui, &it.path, hidden, cache, actions, lang, mark);
            });
            if current {
                if mark.reveal {
                    r.header_response.scroll_to_me(Some(egui::Align::Center));
                }
                r.header_response.clone().on_hover_text(lang.t("Aktueller Ordner der Shell", "Current folder of the shell"));
            }
            r.header_response.clone().context_menu(|ui| menu(ui, actions));
            if r.header_response.double_clicked() {
                actions.push(Action::InsertPath(path_s.clone()));
            }
        } else {
            let r = ui.selectable_label(false, &it.name);
            if r.double_clicked() {
                actions.push(Action::InsertPath(path_s.clone()));
            }
            r.context_menu(|ui| menu(ui, actions));
        }
    }
}

/// Shell-ID eines Profils mit eigenem Programm (`custom:<id>`, aufgelöst wie [`App::shell_by_id`]): Ein eigenes
/// wsl.exe-Programm wird wie eine erkannte WSL-Shell behandelt (Pfadumrechnung, Hooks, Linux-Startordner). Andere
/// bekannte Shells bekommen deren ID, damit eingefügte Pfade passend gequotet werden (sonst bekäme ein
/// cmd-/PowerShell-Profil bash-Quoting mit '…'); sonst bleibt es `custom:<id>`.
pub(super) fn custom_program_shell_id(p: &Profile) -> String {
    let stem = std::path::Path::new(&p.program).file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    match stem.as_str() {
        "wsl" => format!("wsl:{}", shells::wsl_distro(&split_args(&p.args)).unwrap_or_default()),
        // WSL-Starter aus System32: WSL mit der Standard-Distribution (Hooks, WSLENV, Pfade; #261).
        "bash" if shells::is_system_bash(&p.program) => "wsl:".to_string(),
        "pwsh" | "powershell" | "cmd" => stem,
        _ => format!("custom:{}", p.id),
    }
}

/// Argumentzeile in Teile zerlegen (Anführungszeichen "…" werden beachtet).
fn split_args(s: &str) -> Vec<String> {
    let (mut out, mut cur, mut quoted, mut any) = (Vec::new(), String::new(), false, false);
    for c in s.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Taugt der gespeicherte Ordner einer WSL-Sitzung, deren Startordner `wsl.exe` nicht per `--cd` bekommt (eigenes
/// `--cd` des Profils, Distribution unbekannt), als Windows-Startordner? Nie ein Linux-Pfad und nie ein `\\wsl$`-Pfad
/// (den müsste der UI-Thread prüfen); sonst das Windows-Home.
/// Läuft die Shell einer Sitzung in WSL? Eine Regel für den Start von Claude und die Auswahl der Konversation (#302):
/// die Kennung (`wsl…`, auch ohne gefundene Shell) oder das Programm der Shell (eigene Profile mit wsl.exe/bash.exe).
fn shell_is_wsl(spec: Option<&ShellSpec>, shell_id: &str) -> bool {
    let sid = spec.map(|s| s.id.as_str()).unwrap_or(shell_id);
    sid.starts_with("wsl") || spec.is_some_and(shells::is_wsl)
}

/// Distribution einer WSL-Shell: aus der Kennung `wsl:<Distro>`, sonst aus `-d <Distro>` der Argumente; `None` = Standard.
/// `WSLENV` mit `name` (und Kennzeichen wie `/p`) am Ende; ein früherer Eintrag desselben Namens fällt weg (sonst gälte
/// womöglich der ohne `/p`, etwa ein `ZDOTDIR` aus dem Profil).
fn wslenv_with(wslenv: &str, name: &str, flag: &str) -> String {
    // Ein Name mit `:`, `/` oder `=` zerlegte WSLENV (z. B. `X:PATH/l`): nie eintragen (#384 Review).
    if name.is_empty() || name.contains([':', '/', '=']) || name.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return wslenv.to_string();
    }
    let mut parts: Vec<&str> = wslenv.split(':').filter(|p| !p.is_empty() && p.split('/').next() != Some(name)).collect();
    let add = format!("{name}{flag}");
    parts.push(&add);
    parts.join(":")
}

/// Pfadart für einen Linux-Pfad `path` aus `--open`/`--edit` bzw. einen Chat-Ordner: die der Ursprungssitzung, wenn sie
/// POSIX-Pfade kennt (WSL, Git Bash); sonst ist `~`/`~/x` der Windows-Benutzerordner (#392, Entscheidung 9) und ein
/// absoluter Linux-Pfad gilt in der WSL-Standard-Distribution.
fn cli_path_kind(origin: Option<&ShellSpec>, path: &str) -> crate::targetpath::PathKind {
    use crate::targetpath::PathKind;
    let p = path.trim();
    let home_form = p == "~" || p.starts_with("~/") || p.starts_with("~\\");
    match origin.map(PathKind::of) {
        Some(k @ (PathKind::Wsl { .. } | PathKind::Msys { .. })) => k,
        Some(PathKind::Native) => PathKind::Native,
        _ if home_form => PathKind::Windows { cmd: false },
        _ => PathKind::Wsl { distro: None },
    }
}

#[cfg(test)]
mod cli_path_tests {
    use super::*;
    use crate::targetpath::PathKind;

    /// #392: Linux-Pfade aus `--open`/`--edit` und Chat-Ordner gelten in der Pfadart der Ursprungssitzung, sonst WSL.
    #[test]
    fn linux_paths_follow_the_origin_session() {
        let s = |id: &str, prog: &str, args: &[&str]| ShellSpec { id: id.into(), label: id.into(), program: prog.into(), args: args.iter().map(|a| a.to_string()).collect() };
        let deb = s("wsl:Debian", "wsl.exe", &["-d", "Debian"]);
        assert_eq!(cli_path_kind(Some(&deb), "/srv"), PathKind::Wsl { distro: Some("Debian".into()) });
        assert_eq!(cli_path_kind(Some(&deb), "~/x"), PathKind::Wsl { distro: Some("Debian".into()) }, "~ aus WSL = Linux-Home");
        assert_eq!(cli_path_kind(Some(&s("gitbash", r"C:\Program Files\Git\bin\bash.exe", &[])), "/c/x"), PathKind::Msys { root: Some(r"C:\Program Files\Git".into()) });
        assert_eq!(cli_path_kind(None, "/etc/hostname"), PathKind::Wsl { distro: None });
        assert_eq!(cli_path_kind(None, "~"), PathKind::Windows { cmd: false }, "~ ohne Ursprung = Windows-Home");
        if cfg!(windows) {
            let ps = s("powershell", "powershell.exe", &[]);
            assert_eq!(cli_path_kind(Some(&ps), "/srv"), PathKind::Wsl { distro: None }, "Windows-Shell: Standard-Distribution");
            assert_eq!(cli_path_kind(Some(&ps), "~/x"), PathKind::Windows { cmd: false }, "Windows-Shell: ~ = Windows-Home");
            let home = std::env::var("USERPROFILE").unwrap_or_default();
            assert_eq!(cli_path_kind(Some(&ps), "~/x").to_windows("~/x"), Ok(format!("{home}\\x")));
            // Ergebnis für die Sitzung: UNC der Distribution bzw. Laufwerk
            assert_eq!(cli_path_kind(Some(&deb), "/srv/a.txt").to_windows("/srv/a.txt").as_deref(), Ok(r"\\wsl.localhost\Debian\srv\a.txt"));
            assert_eq!(cli_path_kind(Some(&s("wsl:Debian", "wsl.exe", &[])), "/mnt/c/x").to_windows("/mnt/c/x").as_deref(), Ok(r"C:\x"));
        }
    }
}

fn wsl_distro_of(shell: &ShellSpec) -> Option<String> {
    shell.id.strip_prefix("wsl:").filter(|d| !d.is_empty()).map(str::to_string).or_else(|| shells::wsl_distro(&shell.args))
}

fn spawn_cwd_is_windows(cwd: &str) -> bool {
    !cwd.trim().is_empty() && !crate::wsl::looks_linux(cwd.trim()) && !crate::wsl::is_wsl_unc(cwd)
}

/// Ein Argument nach den Regeln von `CommandLineToArgvW` maskieren (Leerzeichen → Anführungszeichen, `\` vor `"`
/// verdoppeln).
fn win_quote_arg(a: &str) -> String {
    if !a.is_empty() && !a.contains([' ', '\t', '\n', '\u{0b}', '"']) {
        return a.to_string();
    }
    let mut out = String::from("\"");
    let mut slashes = 0usize;
    for c in a.chars() {
        match c {
            '\\' => slashes += 1,
            '"' => {
                out.push_str(&"\\".repeat(slashes * 2 + 1));
                out.push('"');
                slashes = 0;
            }
            _ => {
                out.push_str(&"\\".repeat(slashes));
                out.push(c);
                slashes = 0;
            }
        }
    }
    out.push_str(&"\\".repeat(slashes * 2));
    out.push('"');
    out
}

/// PowerShell-Befehl für den Start mit erhöhten Rechten. `Start-Process -ArgumentList` hängt seine Werte nur mit
/// Leerzeichen aneinander; deshalb wird die Kommandozeile hier selbst maskiert (sonst zerfiele z. B.
/// `--cd /home/u/my project` in drei Argumente). Einfache Anführungszeichen (auch die typografischen, die PowerShell
/// ebenso als Begrenzer liest) werden verdoppelt.
fn elevated_script(shell: &ShellSpec, cwd: Option<&str>) -> String {
    let q = |s: &str| {
        let mut o = String::new();
        for c in s.chars() {
            if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
                o.push(c);
            }
            o.push(c);
        }
        o
    };
    let mut script = format!("Start-Process -FilePath '{}' -Verb RunAs", q(&shell.program));
    if !shell.args.is_empty() {
        let line: Vec<String> = shell.args.iter().map(|a| win_quote_arg(a)).collect();
        script += &format!(" -ArgumentList '{}'", q(&line.join(" ")));
    }
    if let Some(c) = cwd {
        script += &format!(" -WorkingDirectory '{}'", q(c));
    }
    script
}

/// Startet die Shell mit erhöhten Rechten in einem externen Fenster (nur Windows, `platform::process::spawn_elevated`).
fn launch_elevated(shell: &ShellSpec, cwd: &str) -> String {
    if !crate::platform::process::ELEVATED {
        return state::t("Start als Administrator wird nur unter Windows unterstützt.", "Starting as administrator is only supported on Windows.").to_string();
    }
    // WSL-Pfade ungeprüft (keine Prüfung im UI-Thread); sonst nur vorhandene Ordner.
    let script = elevated_script(shell, Some(cwd).filter(|c| crate::wsl::is_wsl_unc(c) || crate::pathfmt::dir_exists_quick(c, std::time::Duration::from_millis(400)) == Some(true)));
    if crate::platform::process::spawn_elevated(&script) {
        state::t("Als Administrator in einem eigenen Fenster gestartet (UAC-Abfrage). Dieses Terminal ist nicht eingebettet.", "Started as administrator in its own window (UAC prompt). This terminal is not embedded.").to_string()
    } else {
        state::t("Start als Administrator fehlgeschlagen.", "Starting as administrator failed.").to_string()
    }
}

#[cfg(test)]
mod pane_tests {
    use super::*;
    use eframe::egui::{Event, PointerButton, RawInput};

    #[test]
    fn split_px_keeps_a_minimum_size_and_survives_nan() {
        assert_eq!(split_px(1000.0, 0.5, 120.0, 120.0), 500.0);
        assert_eq!(split_px(1000.0, 0.1, 120.0, 120.0), 120.0, "nicht schmaler als die Mindestbreite");
        assert_eq!(split_px(1000.0, 0.9, 120.0, 120.0), 880.0);
        assert_eq!(split_px(1000.0, 0.5, 120.0, 600.0), 400.0, "Seite b braucht mehr (geschachtelt)");
        assert_eq!(split_px(1000.0, f32::NAN, 120.0, 120.0), 500.0);
        assert_eq!(split_px(200.0, 0.1, 120.0, 120.0), 20.0, "zu wenig Platz für beide: nach Anteil");
        assert_eq!(split_px(-5.0, 0.5, 120.0, 120.0), 0.0);
        // Mindestausdehnung je Teilbaum
        let l = |i| Node::Leaf(i);
        let row = Node::grid(&[1, 2, 3], Dir::Horizontal).unwrap(); // (1 / 2) | 3
        assert_eq!(min_extent(&row, Dir::Horizontal), 2.0 * MIN_PANE_W + GAP);
        assert_eq!(min_extent(&row, Dir::Vertical), 2.0 * MIN_PANE_H + GAP);
        assert_eq!(min_extent(&l(1), Dir::Vertical), MIN_PANE_H);
        // Ziehen: Grenzen, Mindestgröße, nichts bei Länge 0 / NaN
        let near = |a: f32, b: f32| (a - b).abs() < 1e-5;
        assert!(near(drag_ratio(0.5, -1000.0, 1000.0, 120.0, 120.0), 0.12));
        assert!(near(drag_ratio(0.5, 1000.0, 1000.0, 120.0, 120.0), 0.88));
        assert!(near(drag_ratio(0.5, 1000.0, 1000.0, 120.0, 500.0), 0.5), "Seite b braucht 500 px");
        assert!(near(drag_ratio(0.5, 100.0, 1000.0, 120.0, 120.0), 0.6));
        assert_eq!(drag_ratio(0.5, 100.0, 0.0, 120.0, 120.0), 0.5, "minimiertes Fenster");
        assert_eq!(drag_ratio(0.4, f32::NAN, 1000.0, 120.0, 120.0), 0.4);
        assert_eq!(drag_ratio(0.5, -1000.0, 100.0, 120.0, 120.0), crate::layout::RATIO_MIN);
    }

    struct H {
        ctx: egui::Context,
        time: f64,
        entries: BTreeMap<u64, Entry>,
        rects: HashMap<u64, Rect>,
        actions: Vec<Action>,
    }

    impl H {
        fn new(ids: &[(u64, bool)]) -> H {
            let mut entries = BTreeMap::new();
            for (id, chat) in ids {
                let meta = SessionMeta { id: *id, name: format!("s{id}"), custom_name: true, kind: if *chat { SessionKind::Chat } else { SessionKind::Terminal }, ..SessionMeta::default() };
                let e = if *chat { Entry::new_chat(meta, ChatState::new(chatmod::ChatFile::default(), None)) } else { Entry::new(meta) };
                entries.insert(*id, e);
            }
            H { ctx: egui::Context::default(), time: 0.0, entries, rects: HashMap::new(), actions: Vec::new() }
        }

        fn frame(&mut self, layout: &mut Node, detached: bool, events: Vec<Event>) {
            self.time += 0.1;
            let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 700.0))), time: Some(self.time), events, ..Default::default() };
            let theme = crate::theme::all().remove(0);
            let font = FontId::monospace(13.0);
            let (usage, git, tl) = (UsageWatcher::default(), GitWatcher::default(), terminal_links::TLinks::default());
            let (mut want, mut dragging) = (None, None);
            let mut rects = HashMap::new();
            let mut acts = Vec::new();
            let entries = &mut self.entries;
            let _ = self.ctx.run(input, |ctx| {
                egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| {
                    let rect = ui.available_rect_before_wrap();
                    let mut cx = PaneCx {
                        entries,
                        theme: &theme,
                        font: &font,
                        focused: 1,
                        want_focus: &mut want,
                        actions: &mut acts,
                        lang: Lang::En,
                        claude_mode: false,
                        rects: &mut rects,
                        broadcast: false,
                        usage: &usage,
                        git: &git,
                        dragging: &mut dragging,
                        detached,
                        themes: &[],
                        profiles: &[],
                        tl: &tl,
                        alt_wheel: Default::default(),
                        hide_key: "Ctrl+Shift+W",
                        chat_bg: theme.bg,
                    };
                    if detached {
                        show_pane(ui, layout.leaf_ids()[0], rect, &mut cx);
                    } else {
                        show_node(ui, layout, rect, &mut cx);
                    }
                });
            });
            self.rects = rects;
            self.actions.extend(acts);
        }

        fn click(&mut self, layout: &mut Node, detached: bool, p: Pos2) {
            let btn = |pressed| Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Default::default() };
            self.frame(layout, detached, vec![Event::PointerMoved(p)]);
            self.frame(layout, detached, vec![btn(true)]);
            self.frame(layout, detached, vec![btn(false)]);
        }
    }

    /// Mitte des n-ten Kopfzeilenknopfs von rechts (0 = Schließen/Andocken, 1 = Minimieren).
    fn header_btn(r: Rect, n: usize) -> Pos2 {
        Pos2::new(r.max.x - 4.0 - 11.0 - 22.0 * n as f32, r.min.y + 2.0 + (HEADER_H - 2.0) / 2.0)
    }

    #[test]
    fn the_minimize_button_minimizes_terminals_and_chats() {
        let mut h = H::new(&[(1, false), (2, true)]);
        let mut layout = Node::grid(&[1, 2], Dir::Horizontal).unwrap();
        h.frame(&mut layout, false, vec![]);
        h.frame(&mut layout, false, vec![]);
        for id in [1u64, 2] {
            let r = h.rects[&id];
            h.actions.clear();
            h.click(&mut layout, false, header_btn(r, 1));
            assert!(h.actions.iter().any(|a| matches!(a, Action::MinimizePane(x) if *x == id)), "{id}: Minimieren");
            assert!(!h.actions.iter().any(|a| matches!(a, Action::RequestClose(_))), "{id}: nicht schließen");
            h.actions.clear();
            h.click(&mut layout, false, header_btn(r, 0));
            assert!(h.actions.iter().any(|a| matches!(a, Action::RequestClose(x) if *x == id)), "{id}: X schließt weiterhin");
        }
    }

    #[test]
    fn the_reload_button_sits_next_to_minimize_for_terminals_only() {
        let mut h = H::new(&[(1, false), (2, true)]);
        let mut layout = Node::grid(&[1, 2], Dir::Horizontal).unwrap();
        h.frame(&mut layout, false, vec![]);
        h.frame(&mut layout, false, vec![]);
        // Terminal: dritter Knopf von rechts lädt neu (über die Rückfrage), nichts anderes
        let r = h.rects[&1];
        h.click(&mut layout, false, header_btn(r, 2));
        assert!(h.actions.iter().any(|a| matches!(a, Action::RequestReload(1))), "Neu laden");
        assert!(!h.actions.iter().any(|a| matches!(a, Action::Restart(_) | Action::Detach(_) | Action::MinimizePane(_))));
        // Chat: kein Neu laden – an dieser Stelle sitzt „In eigenem Fenster öffnen“
        let r = h.rects[&2];
        h.actions.clear();
        h.click(&mut layout, false, header_btn(r, 2));
        assert!(h.actions.iter().any(|a| matches!(a, Action::Detach(2))));
        assert!(!h.actions.iter().any(|a| matches!(a, Action::RequestReload(_))));
        // Terminal: die übrigen Knöpfe rücken eins weiter (Detach an Stelle 3)
        let r = h.rects[&1];
        h.actions.clear();
        h.click(&mut layout, false, header_btn(r, 3));
        assert!(h.actions.iter().any(|a| matches!(a, Action::Detach(1))));
        // Abgedockt: Andocken, Minimieren, Neu laden
        let mut h = H::new(&[(1, false)]);
        let mut solo = Node::Leaf(1);
        h.frame(&mut solo, true, vec![]);
        h.frame(&mut solo, true, vec![]);
        let r = h.rects[&1];
        h.click(&mut solo, true, header_btn(r, 2));
        assert!(h.actions.iter().any(|a| matches!(a, Action::RequestReload(1))));
        // Abgedockter Chat: kein Neu laden
        let mut h = H::new(&[(2, true)]);
        let mut solo = Node::Leaf(2);
        h.frame(&mut solo, true, vec![]);
        h.frame(&mut solo, true, vec![]);
        let r = h.rects[&2];
        h.click(&mut solo, true, header_btn(r, 2));
        assert!(!h.actions.iter().any(|a| matches!(a, Action::RequestReload(_))));
    }

    #[test]
    fn claude_gates_follow_the_launched_session_not_the_settings() {
        // Gestartet als Claude: Einstellung aus, aber noch nicht neu geladen → weiter Claude (Status, Warteschlange, MCP)
        let mut e = Entry::new(SessionMeta { id: 1, claude: true, ..SessionMeta::default() });
        assert!(e.is_claude());
        e.meta.claude = false;
        assert!(e.is_claude(), "erst das Neu laden macht daraus ein Terminal");
        e.meta.queue.push("p".into());
        e.queue_active = false;
        // Ohne Prozess sendet die Warteschlange nichts, aber nicht wegen der geänderten Einstellung (Prompt bleibt).
        assert!(!e.queue_tick(ClaudeStatus::Working, ClaudeStatus::Idle));
        assert_eq!(e.meta.queue.len(), 1);
        // Umgekehrt: Terminal, Claude eingeschaltet, nicht neu geladen → kein Claude
        let mut t = Entry::new(SessionMeta { id: 2, ..SessionMeta::default() });
        t.meta.claude = true;
        assert!(!t.is_claude());
        // Ein Chat ist nie Claude
        let c = Entry::new_chat(SessionMeta { id: 3, claude: true, kind: SessionKind::Chat, ..SessionMeta::default() }, ChatState::new(chatmod::ChatFile::default(), None));
        assert!(!c.is_claude());
    }

    #[test]
    fn a_detached_window_has_dock_back_minimize_and_reload() {
        let mut h = H::new(&[(1, false)]);
        let mut layout = Node::Leaf(1);
        h.frame(&mut layout, true, vec![]);
        h.frame(&mut layout, true, vec![]);
        let r = h.rects[&1];
        h.click(&mut layout, true, header_btn(r, 1));
        assert!(h.actions.iter().any(|a| matches!(a, Action::MinimizePane(1))));
        h.actions.clear();
        h.click(&mut layout, true, header_btn(r, 0));
        assert!(h.actions.iter().any(|a| matches!(a, Action::Attach(1))));
    }

    #[test]
    fn panes_of_a_deep_split_keep_a_minimum_size() {
        // 6 Bereiche nebeneinander mit lauter 0.1-Anteilen: alle bleiben mindestens MIN_PANE_W breit
        let mut layout = Node::Leaf(6);
        for id in (1..6).rev() {
            layout = Node::Split { dir: Dir::Horizontal, ratio: 0.9, a: Box::new(layout), b: Box::new(Node::Leaf(id)) };
        }
        let ids: Vec<(u64, bool)> = (1..=6).map(|i| (i, false)).collect();
        let mut h = H::new(&ids);
        h.frame(&mut layout, false, vec![]);
        assert_eq!(h.rects.len(), 6);
        for (id, r) in &h.rects {
            assert!(r.width() >= MIN_PANE_W - 0.5, "{id}: {}", r.width());
            assert!(r.width().is_finite() && r.height() > 0.0);
        }
        // Geschachtelt auf Seite b mit lauter 0.1-Anteilen: 1 | (2 | (3 | (4 | (5 | 6))))
        let mut deep_b = Node::Leaf(6);
        for id in (1..6).rev() {
            deep_b = Node::Split { dir: Dir::Horizontal, ratio: 0.1, a: Box::new(Node::Leaf(id)), b: Box::new(deep_b) };
        }
        h.frame(&mut deep_b, false, vec![]);
        assert_eq!(h.rects.len(), 6);
        for (id, r) in &h.rects {
            assert!(r.width() >= MIN_PANE_W - 0.5, "Seite b, {id}: {}", r.width());
        }
        // Quer geschachtelt: (1 / 2 / 3 / 4 / 5 / 6 übereinander) rechts neben 7, Anteil 0.9 → rechts bleibt Mindesthöhe je Bereich
        let mut col = Node::Leaf(6);
        for id in (1..6).rev() {
            col = Node::Split { dir: Dir::Vertical, ratio: 0.1, a: Box::new(Node::Leaf(id)), b: Box::new(col) };
        }
        let mut mixed = Node::Split { dir: Dir::Horizontal, ratio: 0.9, a: Box::new(Node::Leaf(7)), b: Box::new(col) };
        let mut h2 = H::new(&(1..=7).map(|i| (i, false)).collect::<Vec<_>>());
        h2.frame(&mut mixed, false, vec![]);
        for (id, r) in &h2.rects {
            assert!(r.width() >= MIN_PANE_W - 0.5 && r.height() >= MIN_PANE_H - 0.5, "quer, {id}: {r:?}");
        }
        // Unsinniger Anteil (z. B. aus einer alten Datei, ohne Normalisierung): zeichnet trotzdem sinnvoll
        let mut bad = Node::Split { dir: Dir::Vertical, ratio: f32::NAN, a: Box::new(Node::Leaf(1)), b: Box::new(Node::Leaf(2)) };
        h.frame(&mut bad, false, vec![]);
        assert!((h.rects[&1].height() - h.rects[&2].height()).abs() <= 1.0, "NaN → halbe-halbe");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn return_focus_goes_to_the_last_focused_pane_or_the_top_left_one() {
        let row = Node::grid(&[4, 7, 9], Dir::Horizontal).unwrap();
        assert_eq!(return_focus_target(&row, 7), Some(7), "zuletzt fokussiert und sichtbar");
        assert_eq!(return_focus_target(&row, 42), Some(row.leaf_ids()[0]), "unbekannt: erste Pane (oben links)");
        assert_eq!(return_focus_target(&Node::Leaf(3), 0), Some(3));
    }

    /// Ein Bild mit Zentralbereich (Widget `term`) und optional einem Fenster/Tooltip; gibt zurück, ob ein Dialog offen ist.
    fn focus_frame(ctx: &egui::Context, events: Vec<egui::Event>, window: bool, tooltip: bool, request: Option<bool>) -> bool {
        let tid = Id::new("term");
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))), events, ..Default::default() };
        let mut held = false;
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                // `Some(true)`: vor dem Widget fokussieren (alt), `Some(false)`: danach (#238)
                if request == Some(true) {
                    ui.memory_mut(|m| m.request_focus(tid));
                }
                let _ = ui.interact(Rect::from_min_size(Pos2::new(0.0, 100.0), Vec2::new(800.0, 500.0)), tid, Sense::click_and_drag());
                if request == Some(false) {
                    ui.memory_mut(|m| m.request_focus(tid));
                }
            });
            if window {
                egui::Window::new("dlg").show(ctx, |ui| ui.label("x"));
            }
            if tooltip {
                egui::Area::new(Id::new("toast")).order(egui::Order::Tooltip).show(ctx, |ui| ui.label("t"));
            }
            held = focus_held_elsewhere(ctx);
        });
        held
    }

    #[test]
    fn return_focus_respects_open_dialogs_and_survives_the_tab_click() {
        let ctx = egui::Context::default();
        assert!(!focus_frame(&ctx, vec![], false, false, None));
        assert!(!focus_frame(&ctx, vec![], false, true, None), "Toasts/Tooltips halten keinen Fokus");
        assert!(focus_frame(&ctx, vec![], true, false, None), "offenes Fenster");
        let _ = focus_frame(&ctx, vec![], false, false, None);
        assert!(!focus_frame(&ctx, vec![], false, false, None), "Fenster wieder zu");
        // Klick auf einen Reiter oberhalb des Terminals: Druck in einem, Loslassen im nächsten Bild
        let at = Pos2::new(20.0, 20.0);
        let press = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        let tid = Id::new("term");
        for (before, keeps) in [(true, false), (false, true)] {
            let ctx = egui::Context::default();
            let _ = focus_frame(&ctx, vec![egui::Event::PointerMoved(at)], false, false, None);
            let _ = focus_frame(&ctx, vec![press(true)], false, false, None);
            let _ = focus_frame(&ctx, vec![press(false)], false, false, Some(before));
            let _ = focus_frame(&ctx, vec![], false, false, None);
            assert_eq!(ctx.memory(|m| m.has_focus(tid)), keeps, "Fokus vor dem Widget angefordert: {before}");
        }
    }

    #[test]
    fn wsl_spawn_cwd_fallbacks() {
        // Profil mit eigenem `--cd`: ein Linux-/WSL-Ordner wird nie zum Windows-Startordner.
        assert!(spawn_cwd_is_windows(r"C:\Work"));
        assert!(!spawn_cwd_is_windows("/home/u"));
        assert!(!spawn_cwd_is_windows("~"));
        assert!(!spawn_cwd_is_windows(r"\\wsl$\Ubuntu\home"));
        assert!(!spawn_cwd_is_windows(""));
        let mut args = vec!["--cd".to_string(), "/srv".to_string()];
        assert_eq!(crate::wsl::apply_cd(&mut args, "/home/u", Some("Ubuntu")), crate::wsl::Cd::Windows);
        // Gespeicherter Ordner mit `"` (z. B. per OSC 7 aus dem Terminal): beim Wiederherstellen ~ statt Abbruch.
        let mut args = vec!["-d".to_string(), "Ubuntu".to_string()];
        assert!(matches!(crate::wsl::apply_cd(&mut args, "/tmp/x\" echo pwned; #", Some("Ubuntu")), crate::wsl::Cd::Invalid(_)));
        assert_eq!(crate::wsl::apply_cd(&mut args, "~", Some("Ubuntu")), crate::wsl::Cd::Linux);
        assert_eq!(args, vec!["--cd", "~", "-d", "Ubuntu"]);
        // cwd_now: WSL ohne Ordner = ~, andere Shells = Windows-Home
        let mut e = Entry::new(SessionMeta { shell_id: "wsl:Ubuntu".into(), ..SessionMeta::default() });
        assert_eq!(e.cwd_now(), "~");
        e.meta.shell_id = "powershell".into();
        assert_eq!(e.cwd_now(), home_dir());
    }

    #[test]
    fn elevated_start_quotes_each_argument() {
        let mut args = vec!["-d".to_string(), "Ubuntu".to_string()];
        assert_eq!(crate::wsl::apply_cd(&mut args, "/home/u/my project", Some("Ubuntu")), crate::wsl::Cd::Linux);
        let shell = ShellSpec { id: "wsl:Ubuntu".into(), label: "x".into(), program: "wsl.exe".into(), args };
        let s = elevated_script(&shell, Some(r"C:\Users\o'neil"));
        assert_eq!(s, r#"Start-Process -FilePath 'wsl.exe' -Verb RunAs -ArgumentList '--cd "/home/u/my project" -d Ubuntu' -WorkingDirectory 'C:\Users\o''neil'"#);
        // CommandLineToArgvW-Regeln: Anführungszeichen und Backslashes davor, abschließende Backslashes
        assert_eq!(win_quote_arg("plain"), "plain");
        assert_eq!(win_quote_arg(""), "\"\"");
        assert_eq!(win_quote_arg(r#"a "b""#), r#""a \"b\"""#);
        assert_eq!(win_quote_arg(r"C:\a b\"), r#""C:\a b\\""#);
        assert_eq!(win_quote_arg(r#"x\"y"#), r#""x\\\"y""#);
        // Typografische einfache Anführungszeichen beenden in PowerShell ebenfalls den String: verdoppeln.
        let shell = ShellSpec { id: "pwsh".into(), label: "x".into(), program: "pwsh.exe".into(), args: vec!["-Command".into(), "echo \u{2019}; calc".into()] };
        assert!(elevated_script(&shell, None).contains("'-Command \"echo \u{2019}\u{2019}; calc\"'"));
    }

    fn sessions(ids: &[(u64, bool)]) -> BTreeMap<u64, Entry> {
        ids.iter().map(|(id, min)| (*id, Entry::new(SessionMeta { id: *id, minimized: *min, ..SessionMeta::default() }))).collect()
    }

    fn minimized_ids(e: &BTreeMap<u64, Entry>) -> Vec<u64> {
        e.iter().filter(|(_, e)| e.meta.minimized).map(|(id, _)| *id).collect()
    }

    #[test]
    fn showing_a_minimized_session_minimizes_the_visible_panes() {
        let h = |a: u64, b: u64| Node::Split { dir: Dir::Horizontal, ratio: 0.5, a: Box::new(Node::Leaf(a)), b: Box::new(Node::Leaf(b)) };
        // 1 | 2 sichtbar, 3 minimiert (mit veraltetem `slot` aus 2.2.0)
        let mut e = sessions(&[(1, false), (2, false), (3, true), (4, false)]);
        e.get_mut(&3).unwrap().meta.slot = Some(serde_json::json!({"sibling": [1]}));
        let mut l = Some(h(1, 2));
        show_session(&mut l, &mut e, 3, 1);
        assert_eq!(l, Some(Node::Leaf(3)), "allein");
        assert_eq!(minimized_ids(&e), vec![1, 2], "die bisher sichtbaren sind minimiert");
        assert!(e[&3].meta.slot.is_none(), "veralteter Platz gelöscht");
        // nur weggewechselte Sitzung (4): ersetzt den fokussierten Bereich, minimiert nichts
        show_session(&mut l, &mut e, 4, 3);
        assert_eq!(l, Some(Node::Leaf(4)));
        assert_eq!(minimized_ids(&e), vec![1, 2]);
        // zurückholen tauscht: 1 allein, 4 minimiert
        show_session(&mut l, &mut e, 1, 4);
        assert_eq!(l, Some(Node::Leaf(1)));
        assert_eq!(minimized_ids(&e), vec![2, 4]);
        // schon sichtbar: nichts ändert sich
        show_session(&mut l, &mut e, 1, 1);
        assert_eq!(minimized_ids(&e), vec![2, 4]);
        // leerer Hub: einziger Bereich, niemand minimiert
        let mut none = None;
        show_session(&mut none, &mut e, 2, 0);
        assert_eq!(none, Some(Node::Leaf(2)));
        assert_eq!(minimized_ids(&e), vec![4]);
        // unbekannte Sitzung: nichts
        show_session(&mut l, &mut e, 99, 1);
        assert_eq!(l, Some(Node::Leaf(1)));
    }

    #[test]
    fn panes_that_get_into_the_layout_otherwise_are_not_minimized() {
        // z. B. per Ziehen oder Ordner-Layout ins Layout gekommen, ohne `activate`
        let mut e = sessions(&[(1, true), (2, true), (3, true)]);
        e.get_mut(&2).unwrap().meta.slot = Some(serde_json::json!(17));
        let l = Node::Split { dir: Dir::Vertical, ratio: 0.5, a: Box::new(Node::Leaf(1)), b: Box::new(Node::Leaf(2)) };
        unminimize_shown(Some(&l), &mut e);
        assert_eq!(minimized_ids(&e), vec![3], "nur was nicht im Layout steht, bleibt minimiert");
        assert!(e[&2].meta.slot.is_none());
        unminimize_shown(None, &mut e);
        assert_eq!(minimized_ids(&e), vec![3], "leerer Hub: nichts ändert sich");
    }

    #[test]
    fn show_session_counts_the_minimized_panes() {
        let h = |a: u64, b: u64| Node::Split { dir: Dir::Horizontal, ratio: 0.5, a: Box::new(Node::Leaf(a)), b: Box::new(Node::Leaf(b)) };
        let mut e = sessions(&[(1, false), (2, false), (3, true)]);
        let mut l = Some(h(1, 2));
        assert_eq!(show_session(&mut l, &mut e, 3, 1), 2);
        assert_eq!(show_session(&mut l, &mut e, 3, 3), 0, "schon sichtbar");
        assert_eq!(show_session(&mut l, &mut e, 1, 3), 1);
        assert_eq!(show_session(&mut l, &mut e, 99, 1), 0, "unbekannt");
    }

    #[test]
    fn agent_placement_minimizes_displaced_panes() {
        use crate::layout::Placement;
        let free = |_: u64| false;
        let h = |a: u64, b: u64| Node::Split { dir: Dir::Horizontal, ratio: 0.5, a: Box::new(Node::Leaf(a)), b: Box::new(Node::Leaf(b)) };
        // 1 | 2 sichtbar, 3 minimiert, 4 nur weggewechselt
        let mut e = sessions(&[(1, false), (2, false), (3, true), (4, false)]);
        // replace: die minimierte 3 ersetzt nur den aktiven Bereich 2; 2 ist danach minimiert (Entscheidung des Nutzers)
        let mut l = Some(h(1, 2));
        assert_eq!(place_session(&mut l, &mut e, 3, 2, Placement::Replace, &free), 1);
        assert_eq!(l, Some(h(1, 3)));
        assert_eq!(minimized_ids(&e), vec![2]);
        // split_right am Bereich 1: nur 1 wird geteilt, nichts minimiert
        assert_eq!(place_session(&mut l, &mut e, 4, 1, Placement::SplitRight, &free), 0);
        assert_eq!(l.as_ref().unwrap().leaf_ids(), vec![1, 4, 3]);
        // solo: alle anderen minimiert; die gezeigte 2 ist es nicht mehr
        assert_eq!(place_session(&mut l, &mut e, 2, 1, Placement::Solo, &free), 3);
        assert_eq!(l, Some(Node::Leaf(2)));
        assert_eq!(minimized_ids(&e), vec![1, 3, 4]);
        // unbekannte Sitzung: nichts
        assert_eq!(place_session(&mut l, &mut e, 99, 2, Placement::Solo, &free), 0);
        assert_eq!(l, Some(Node::Leaf(2)));
    }

    #[test]
    fn agent_placement_keeps_approval_and_chain_panes() {
        use crate::layout::Placement;
        let h = |a: u64, b: u64| Node::Split { dir: Dir::Horizontal, ratio: 0.5, a: Box::new(Node::Leaf(a)), b: Box::new(Node::Leaf(b)) };
        // 1 wartet auf eine Freigabe, 2 frei, 5 minimiert
        let mut e = sessions(&[(1, false), (2, false), (5, true)]);
        let prot = |x: u64| x == 1;
        let mut l = Some(h(1, 2));
        // replace auf den geschützten aktiven Bereich: geteilt statt ersetzt, nichts minimiert
        assert_eq!(place_session(&mut l, &mut e, 5, 1, Placement::Replace, &prot), 0);
        assert_eq!(l.as_ref().unwrap().leaf_ids(), vec![1, 5, 2]);
        assert!(minimized_ids(&e).is_empty());
        // solo: 1 bleibt sichtbar, nur 5 wird minimiert
        assert_eq!(place_session(&mut l, &mut e, 2, 1, Placement::Solo, &prot), 1);
        assert_eq!(l.as_ref().unwrap().leaf_ids(), vec![1, 2]);
        assert_eq!(minimized_ids(&e), vec![5]);
    }

    #[test]
    fn agent_placement_keeps_the_users_focus() {
        let h = |a: u64, b: u64| Node::Split { dir: Dir::Horizontal, ratio: 0.5, a: Box::new(Node::Leaf(a)), b: Box::new(Node::Leaf(b)) };
        assert_eq!(keep_focus(Some(&h(1, 9)), 1, 9), 1, "fokussierter Bereich noch da: Fokus bleibt");
        assert_eq!(keep_focus(Some(&h(9, 3)), 1, 9), 9, "fokussierter Bereich ersetzt: Fokus zur neuen Sitzung");
        assert_eq!(keep_focus(Some(&Node::Leaf(9)), 0, 9), 9, "vorher leerer Hub");
        assert_eq!(keep_focus(None, 1, 9), 9);
    }

    #[test]
    fn agent_anchor_is_the_callers_pane_if_visible() {
        let l = Node::Split { dir: Dir::Horizontal, ratio: 0.5, a: Box::new(Node::Leaf(1)), b: Box::new(Node::Leaf(2)) };
        assert_eq!(agent_anchor(Some(&l), Some(2), 1), 2, "eigene Sitzung sichtbar");
        assert_eq!(agent_anchor(Some(&l), Some(7), 1), 1, "eigene Sitzung nicht im Layout: Fokus des Nutzers");
        assert_eq!(agent_anchor(Some(&l), None, 1), 1, "Agent außerhalb von Paneldeck");
        assert_eq!(agent_anchor(None, Some(2), 0), 0);
    }

    #[test]
    fn ctrl_tab_cycles_through_all_sessions_including_minimized() {
        assert_eq!(cycle_target(&[1, 2, 3], 1, 1), Some(2));
        assert_eq!(cycle_target(&[1, 2, 3], 3, 1), Some(1));
        assert_eq!(cycle_target(&[1, 2, 3], 1, -1), Some(3));
        assert_eq!(cycle_target(&[1, 2, 3], 9, 1), Some(1), "Fokus nicht in der Liste: die erste");
        assert_eq!(cycle_target(&[1, 2, 3], 9, -1), Some(1));
        assert_eq!(cycle_target(&[], 1, 1), None);
        // Minimierte Sitzung: im fokussierten Bereich gezeigt (replace), die Teilung bleibt, der verdrängte wird minimiert.
        let h = |a: u64, b: u64| Node::Split { dir: Dir::Horizontal, ratio: 0.5, a: Box::new(Node::Leaf(a)), b: Box::new(Node::Leaf(b)) };
        let mut e = sessions(&[(1, false), (2, false), (3, true)]);
        let mut l = Some(h(1, 2));
        let next = cycle_target(&[1, 2, 3], 2, 1).unwrap();
        assert_eq!(next, 3);
        assert_eq!(place_session(&mut l, &mut e, next, 2, crate::layout::Placement::Replace, &|_| false), 1);
        assert_eq!(l, Some(h(1, 3)));
        assert_eq!(minimized_ids(&e), vec![2]);
    }

    #[test]
    fn minimized_hint_in_both_languages() {
        assert_eq!(minimized_hint(1, Lang::En), "1 pane minimized");
        assert_eq!(minimized_hint(3, Lang::En), "3 panes minimized");
        assert_eq!(minimized_hint(1, Lang::De), "1 Bereich minimiert");
        assert_eq!(minimized_hint(2, Lang::De), "2 Bereiche minimiert");
    }

    #[test]
    fn editor_readonly_survives_file_switch() {
        // Nicht-UTF-8-Datei (Latin-1) bleibt nach Wechsel zu anderer Datei und zurück schreibgeschützt.
        let dir = std::env::temp_dir().join(format!("edtest-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".claude")).unwrap();
        std::fs::write(dir.join("CLAUDE.md"), b"Gr\xfc\xdfe").unwrap();
        std::fs::write(dir.join(".claude").join("settings.json"), "{}").unwrap();
        let mut e = Editor::new(&dir.to_string_lossy());
        assert!(e.readonly);
        e.select(1);
        assert!(!e.readonly);
        e.select(0);
        assert!(e.readonly, "Schreibschutz ging beim Dateiwechsel verloren");
        e.text.push('x');
        e.save();
        assert_eq!(std::fs::read(dir.join("CLAUDE.md")).unwrap(), b"Gr\xfc\xdfe");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn splits_arguments() {
        assert_eq!(split_args(r#"-d Ubuntu --cd "C:\Program Files" x"#), vec!["-d", "Ubuntu", "--cd", "C:\\Program Files", "x"]);
        assert!(split_args("   ").is_empty());
        assert_eq!(split_args("\"\""), vec![""]);
    }
}

/// Ziel beim Zurück in den Terminal Hub (#238): die zuletzt fokussierte Sitzung, wenn sie im Layout steht, sonst die
/// erste Pane (oben links).
fn return_focus_target(layout: &Node, focused: u64) -> Option<u64> {
    let ids = layout.leaf_ids();
    if ids.contains(&focused) { Some(focused) } else { ids.first().copied() }
}

/// Hält gerade ein Dialog, Fenster oder Popup den Fokus bzw. ist eines offen? Dann holt sich ein Terminal den Fokus
/// nicht von selbst (#238). Fenster und Modale liegen in den Ebenen `Middle`/`Foreground`; Tooltips und Toasts
/// (`Tooltip`) zählen nicht.
fn focus_held_elsewhere(ctx: &egui::Context) -> bool {
    egui::Popup::is_any_open(ctx)
        || ctx.memory(|m| {
            m.top_modal_layer().is_some() || m.areas().visible_layer_ids().iter().any(|l| matches!(l.order, egui::Order::Middle | egui::Order::Foreground))
        })
}

/// „+“-Knopf am Rand eines Terminals: erscheint, wenn die Maus nahe an einem Rand ist, und teilt dort den Bereich.
/// `bottom = false`: nicht am unteren Rand (ein Chat hat dort sein Eingabefeld).
fn edge_buttons(ui: &mut Ui, id: u64, body: Rect, cx: &mut PaneCx, bottom: bool) {
    const ZONE: f32 = 30.0;
    let Some(p) = ui.input(|i| i.pointer.hover_pos()) else { return };
    if !body.contains(p) || cx.dragging.is_some() {
        return;
    }
    let lang = cx.lang;
    // nächster Rand: (Abstand, Seite)
    let cands = [(p.x - body.min.x, 0u8), (body.max.x - p.x, 1), (p.y - body.min.y, 2), (if bottom { body.max.y - p.y } else { f32::MAX }, 3)];
    let (dist, side) = cands.into_iter().fold((f32::MAX, 0u8), |a, b| if b.0 < a.0 { b } else { a });
    if dist > ZONE {
        return;
    }
    let (center, dir, before, tip) = match side {
        0 => (Pos2::new(body.min.x + 16.0, body.center().y), Dir::Horizontal, true, lang.t("Links daneben ein neues Terminal öffnen", "Open a new terminal to the left")),
        1 => (Pos2::new(body.max.x - 16.0, body.center().y), Dir::Horizontal, false, lang.t("Rechts daneben ein neues Terminal öffnen", "Open a new terminal to the right")),
        2 => (Pos2::new(body.center().x, body.min.y + 16.0), Dir::Vertical, true, lang.t("Darüber ein neues Terminal öffnen", "Open a new terminal above")),
        _ => (Pos2::new(body.center().x, body.max.y - 16.0), Dir::Vertical, false, lang.t("Darunter ein neues Terminal öffnen", "Open a new terminal below")),
    };
    let rect = Rect::from_center_size(center, Vec2::splat(26.0));
    // Bei gedrückter Maustaste nur anzeigen, wenn der Druck auf dem Knopf begann (sonst blitzt er beim Textmarkieren auf –
    // und ein Klick ginge verloren, weil der Knopf beim Drücken verschwände).
    if ui.input(|i| i.pointer.any_down()) && !ui.input(|i| i.pointer.press_origin()).is_some_and(|o| rect.contains(o)) {
        return;
    }
    let resp = ui.interact(rect, Id::new(("edge", id, side)), Sense::click());
    let appear = style::anim(ui.ctx(), Id::new(("edge_a", id, side)), true, style::D_FAST);
    let hover = style::anim(ui.ctx(), Id::new(("edge_h", id, side)), resp.hovered(), style::D_FAST);
    let r = 11.0 + 2.0 * hover;
    let painter = ui.painter();
    painter.circle_filled(center, r, style::brand().gamma_multiply((0.55 + 0.4 * hover) * appear));
    painter.circle_stroke(center, r, Stroke::new(1.0_f32, Color32::from_black_alpha((80.0 * appear) as u8)));
    style::paint_icon(painter, SIcon::Plus, center, Color32::from_rgba_unmultiplied(255, 255, 255, (255.0 * appear) as u8));
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if resp.clicked() {
        cx.actions.push(Action::SplitAt(id, dir, before));
    }
    resp.on_hover_text(tip);
}

/// Geometrie des Ordnerbaums der letzten Frame (für die Ablage-Linie beim Ziehen).
#[derive(Default)]
struct TreeGeo {
    sess: std::collections::HashMap<u64, Rect>,
    head: std::collections::HashMap<String, Rect>,
    full: std::collections::HashMap<String, Rect>,
    x: std::collections::HashMap<String, f32>,
    right: f32,
}

/// Status eines Chats als Sitzungsstatus: Freigabe/Frage offen → `Permission` (Seitenleiste „braucht dich“, MCP
/// `approval`, externe Agenten schreiben dann nicht in den Chat), Runde läuft → `Working`, sonst `Idle`.
fn chat_claude_status(s: chatmod::ChatStatus) -> ClaudeStatus {
    match s {
        chatmod::ChatStatus::Approval => ClaudeStatus::Permission,
        chatmod::ChatStatus::Working => ClaudeStatus::Working,
        chatmod::ChatStatus::Ready => ClaudeStatus::Idle,
    }
}

/// Die App als Hub der Werkzeug-Schleife (`chat::driver`): Ausführung über den MCP-Ausführer mit dem Chat als
/// Aufrufer, nächste Anfrage über das Backend des Hubs.
struct ChatHost<'a> {
    app: &'a mut App,
    ctx: &'a egui::Context,
}

impl chatmod::driver::ChatToolHost for ChatHost<'_> {
    fn chat(&mut self, id: u64) -> Option<&mut ChatState> {
        self.app.entries.get_mut(&id).and_then(|e| e.chat.as_mut())
    }
    fn policy(&self, id: u64) -> chatmod::tools::ToolPolicy {
        self.app.chat_policy(id)
    }
    fn execute(&mut self, chat: u64, call: &chatmod::store::ToolCall) -> std::sync::mpsc::Receiver<chatmod::tools::ToolResult> {
        // Agenten-Werkzeuge (#338; `run_command` nur ohne `session`) laufen im Hub selbst, nicht über den MCP-Ausführer.
        if chatmod::agent::is_local(call) {
            let max = self.app.entries.get(&chat).and_then(|e| e.chat.as_ref()).map_or(0, |c| c.result_cap_for(&call.name));
            return self.app.agent_call(self.ctx, chat, call, max);
        }
        // `screenshot` für ein Modell mit Bildverständnis (#357): das Bild kommt in den Chat.
        let sink = if call.name == "screenshot" { self.app.chat_tool_image_sink(chat, call) } else { None };
        let rx = self.app.mcp_call_chat(self.ctx, chat, &call.name, &call.arguments);
        match sink {
            Some(s) => chatmod::images::forward_screenshot(rx, s),
            None => rx,
        }
    }
    fn request(&mut self, id: u64) {
        self.app.chat_next_request(self.ctx, id);
    }
    fn session_name(&self, id: u64) -> Option<String> {
        self.app.entries.get(&id).map(|e| e.display_name())
    }
    fn lang(&self) -> crate::state::Lang {
        self.app.settings.language
    }
}

#[cfg(test)]
mod chat_status_tests {
    use super::*;

    #[test]
    fn only_a_level_question_of_the_user_sends_the_draft_afterwards() {
        use chatmod::LevelAsker::{Agent, User};
        let agent = ChatSendOrigin::Agent { caller: Some(5), note: None };
        // Ein Agent löst die Frage aus → nach der Antwort wird nichts gesendet (der Entwurf gehört dem Nutzer).
        assert_eq!(level_asker(None, &agent), Agent);
        assert!(!send_after_level_choice(Some(Agent)));
        // Der Nutzer löst sie aus (oder drückt selbst „Senden“, während die Frage eines Agenten offen ist) → sein Entwurf geht los.
        assert_eq!(level_asker(None, &ChatSendOrigin::User), User);
        assert_eq!(level_asker(Some(Agent), &ChatSendOrigin::User), User);
        // Ein späterer Agent macht die Frage des Nutzers nicht zu seiner.
        assert_eq!(level_asker(Some(User), &agent), User);
        assert!(send_after_level_choice(Some(User)));
        assert!(!send_after_level_choice(None), "keine Frage offen (z. B. im Reiter Modelle gewählt): nichts senden");
    }

    #[test]
    fn chat_status_maps_to_session_status() {
        assert_eq!(chat_claude_status(chatmod::ChatStatus::Approval), ClaudeStatus::Permission);
        assert_eq!(chat_claude_status(chatmod::ChatStatus::Working), ClaudeStatus::Working);
        assert_eq!(chat_claude_status(chatmod::ChatStatus::Ready), ClaudeStatus::Idle);
        // Ein Chat mit offener Frage „braucht dich“
        let mut e = Entry::new_chat(SessionMeta { kind: SessionKind::Chat, ..SessionMeta::default() }, ChatState::new(chatmod::ChatFile::default(), None));
        e.status = chat_claude_status(chatmod::ChatStatus::Approval);
        assert!(e.needs_you());
    }
}

/// Wer die Frage nach der Stufe auslöst: der Nutzer, sobald er selbst sendet (auch wenn ein Agent sie vorher auslöste);
/// sonst ein Agent.
fn level_asker(open: Option<chatmod::LevelAsker>, origin: &ChatSendOrigin) -> chatmod::LevelAsker {
    match (open, origin) {
        (_, ChatSendOrigin::User) | (Some(chatmod::LevelAsker::User), _) => chatmod::LevelAsker::User,
        _ => chatmod::LevelAsker::Agent,
    }
}

/// Geht nach der Antwort auf die Frage nach der Stufe der Entwurf los? Nur, wenn der Nutzer sie mit „Senden“ auslöste.
fn send_after_level_choice(asker: Option<chatmod::LevelAsker>) -> bool {
    asker == Some(chatmod::LevelAsker::User)
}

/// Woher eine Chat-Nachricht kommt ([`App::chat_send`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ChatSendOrigin {
    /// Eingabefeld des Chats.
    User,
    /// Externer Agent über MCP (`send_chat_message`): `caller` = seine Hub-Sitzung (`None` = außerhalb von Paneldeck).
    /// Die Werkzeuge der ausgelösten Runde dürfen nicht in diese Sitzung (und nicht in die Kette davor) schreiben
    /// (`ChatState::origin_sessions`).
    Agent { caller: Option<u64>, note: Option<String> },
}

/// Ergebnis von [`App::chat_send`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ChatSendResult {
    /// Gesendet, die Antwort läuft.
    Sent,
    /// Nicht gesendet: die einmalige Frage nach der Stufe des Modells ist offen (für Agenten `permission_pending`).
    LevelQuestion,
    /// Nicht gesendet: eine Antwort läuft, Werkzeuge laufen oder eine Freigabe ist offen.
    Busy,
    /// Nichts zu senden.
    Empty,
    /// Keine Chat-Sitzung mit dieser Id.
    NotAChat,
}

#[cfg(test)]
mod shortcut_match_tests {
    //! Feste und eigene Kürzel (#254): genaue Modifikatoren, physische Zifferntasten, Ziele; Knöpfe im Bereichskopf (#259).
    use super::*;

    fn key(k: Key, physical: Key, m: Modifiers) -> Event {
        Event::Key { key: k, physical_key: Some(physical), pressed: true, repeat: false, modifiers: m }
    }

    /// Ein Bild mit `events`; `f` prüft/verbraucht. Liefert (Ergebnis, übrig gebliebene Tasten-Ereignisse).
    fn run(events: Vec<Event>, f: impl Fn(&mut egui::InputState) -> bool) -> (bool, usize) {
        let ctx = egui::Context::default();
        let (mut hit, mut left) = (false, 0);
        let _ = ctx.run(egui::RawInput { events, ..Default::default() }, |ctx| {
            hit = ctx.input_mut(|i| f(i));
            left = ctx.input(|i| i.events.iter().filter(|e| matches!(e, Event::Key { .. })).count());
        });
        (hit, left)
    }

    const CTRL: Modifiers = Modifiers { ctrl: true, command: true, alt: false, shift: false, mac_cmd: false };

    #[test]
    fn extra_shift_or_altgr_is_another_shortcut() {
        // Strg+1 = Reiter „Terminal Hub“
        assert_eq!(run(vec![key(Key::Num1, Key::Num1, CTRL)], |i| consume_key_exact(i, Modifiers::CTRL, Key::Num1, true)), (true, 0));
        // Strg+Umschalt+1 (Projekt 1) und Strg+AltGr+2 („²“ auf deutschen Tastaturen) lösen Strg+1/2 nicht aus
        let cs = CTRL | Modifiers::SHIFT;
        assert_eq!(run(vec![key(Key::Exclamationmark, Key::Num1, cs)], |i| consume_key_exact(i, Modifiers::CTRL, Key::Num1, true)), (false, 1));
        let altgr = CTRL | Modifiers::ALT;
        assert_eq!(run(vec![key(Key::Num2, Key::Num2, altgr)], |i| consume_key_exact(i, Modifiers::CTRL, Key::Num2, true)), (false, 1));
        // … wohl aber Strg+Umschalt+1 das Projekt-Kürzel, auch wenn die Belegung „!“ oder „/“ liefert (physisch)
        for logical in [Key::Exclamationmark, Key::Slash, Key::Num1] {
            assert_eq!(run(vec![key(logical, Key::Num1, cs)], |i| consume_key_exact(i, Modifiers::CTRL | Modifiers::SHIFT, Key::Num1, true)), (true, 0), "{logical:?}");
        }
        // Eigene Kürzel logisch: Strg+Umschalt+T nicht durch Strg+Alt+Umschalt+T
        assert_eq!(run(vec![key(Key::T, Key::T, cs | Modifiers::ALT)], |i| consume_key_exact(i, Modifiers::CTRL | Modifiers::SHIFT, Key::T, false)), (false, 1));
        assert_eq!(run(vec![key(Key::T, Key::T, cs)], |i| consume_key_exact(i, Modifiers::CTRL | Modifiers::SHIFT, Key::T, false)), (true, 0));
        // Ohne `physical` zählt nur die logische Taste
        assert_eq!(run(vec![key(Key::Exclamationmark, Key::Num1, cs)], |i| consume_key_exact(i, Modifiers::CTRL | Modifiers::SHIFT, Key::Num1, false)), (false, 1));
    }

    #[test]
    fn focus_target_only_where_a_pane_lies() {
        let mut rects = HashMap::new();
        rects.insert(1, Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(400.0, 300.0)));
        rects.insert(2, Rect::from_min_size(Pos2::new(406.0, 0.0), Vec2::new(400.0, 300.0)));
        rects.insert(3, Rect::from_min_size(Pos2::new(406.0, 306.0), Vec2::new(400.0, 300.0)));
        assert_eq!(focus_target(&rects, 1, Nav::Right), Some(2));
        assert_eq!(focus_target(&rects, 1, Nav::Left), None, "links liegt nichts: Taste bleibt beim Terminal");
        assert_eq!(focus_target(&rects, 1, Nav::Up), None);
        assert_eq!(focus_target(&rects, 2, Nav::Down), Some(3));
        assert_eq!(focus_target(&rects, 3, Nav::Left), Some(1));
        assert_eq!(focus_target(&HashMap::new(), 1, Nav::Right), None);
    }

    #[test]
    fn narrow_pane_headers_keep_their_buttons_inside() {
        let all = pane_buttons(false, true, true);
        assert_eq!(all, vec![PaneBtn::Close, PaneBtn::Minimize, PaneBtn::Reload, PaneBtn::Detach, PaneBtn::SplitDown, PaneBtn::SplitRight, PaneBtn::Link]);
        assert_eq!(pane_buttons(true, false, true), vec![PaneBtn::Close, PaneBtn::Minimize], "abgedockt");
        for w in [MIN_PANE_W, 140.0, 174.0, 200.0, 260.0, 400.0, 1000.0] {
            let n = pane_buttons_fit(w, all.len());
            assert!(n >= 2, "Schließen und Minimieren immer ({w})");
            // Knöpfe plus Luft und ein Stück Name passen in den Kopf: nichts ragt in den Nachbarn
            if n > 2 {
                assert!(PANE_BTN_W * n as f32 + 16.0 + 60.0 <= w + 0.01, "{n} Knöpfe in {w}");
            }
        }
        assert_eq!(pane_buttons_fit(1000.0, all.len()), all.len(), "breit: alle");
        assert_eq!(pane_buttons_fit(MIN_PANE_W, all.len()), 2);
    }
}
