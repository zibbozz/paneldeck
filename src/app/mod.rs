use crate::clip;
use crate::gitinfo::{GitInfo, GitWatcher};
use crate::gitops::{self, Change, DiffWatcher};
use crate::history::{self, HistoryItem};
use crate::hooks::{self, ClaudeStatus, HookMsg, HookServer, IpcCmd};
use crate::keys;
use crate::usage::{self, fmt_tokens, Usage, UsageWatcher};
use crate::layout::{Dir, Node};
use crate::notify;
use crate::pathfmt;
use crate::prompts::{self, Snippet};
use crate::search::{self, SearchState};
use crate::style::{self, Icon as SIcon};
use crate::templates::{self, Template, TplItem};
use crate::tray::{Tray, TrayCmd};
use crate::shells::{self, ShellSpec};
use crate::state::{self, Lang, Profile, Project, SavedState, SessionMeta, Settings};
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

mod data;
mod dialogs;
mod dragdrop;
#[cfg(test)]
mod glyph_tests;
mod mcp;
mod panels;
mod planning_ui;
mod settings;
#[cfg(feature = "readme-shots")]
mod shot_start;
mod sidebar;
mod sysprompt_ui;
mod tasks_ui;
mod terminal_links;
mod titlebar;
mod toasts;
mod winchrome;

const PALETTE: [[u8; 3]; 8] = [
    [0x61, 0xaf, 0xef],
    [0x98, 0xc3, 0x79],
    [0xe5, 0xc0, 0x7b],
    [0xe0, 0x6c, 0x75],
    [0xc6, 0x78, 0xdd],
    [0x56, 0xb6, 0xc2],
    [0xd9, 0x77, 0x57],
    [0x88, 0x88, 0x88],
];

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
}

impl Entry {
    fn new(meta: SessionMeta) -> Self {
        Entry { meta, live: None, error: None, status: ClaudeStatus::Unknown, status_msg: String::new(), attention: false, live_cwd: String::new(), hook_seen: false, queue_active: false, perm_since: None, budget_warned: false, shell_eff: String::new() }
    }
    /// Nächsten wartenden Prompt senden, wenn Claude von beschäftigt auf bereit wechselt.
    /// Gibt `true` zurück, wenn die Warteschlange gerade vollständig abgearbeitet wurde.
    fn queue_tick(&mut self, old: ClaudeStatus, new: ClaudeStatus) -> bool {
        if !self.meta.claude || self.meta.queue_paused || self.exited() {
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
        if let Some(s) = &self.live {
            s.send_text(&text, true);
            self.status = ClaudeStatus::Working;
            self.queue_active = true;
        } else {
            self.meta.queue.insert(0, text);
        }
    }

    fn eff_shell(&self) -> &str {
        if self.shell_eff.is_empty() { &self.meta.shell_id } else { &self.shell_eff }
    }

    fn cwd_now(&self) -> String {
        if !self.live_cwd.is_empty() {
            self.live_cwd.clone()
        } else if self.meta.cwd.is_empty() {
            home_dir()
        } else {
            self.meta.cwd.clone()
        }
    }
    fn status_label(&self, lang: Lang) -> &'static str {
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
        if self.exited() {
            return Color32::from_gray(110);
        }
        match self.status {
            ClaudeStatus::Unknown | ClaudeStatus::Idle => Color32::from_rgb(0x4c, 0xc2, 0x6a),
            ClaudeStatus::Working => {
                let k = 0.55 + 0.45 * ((t * 4.0).sin() as f32 * 0.5 + 0.5);
                Color32::from_rgb(0xd9, 0x77, 0x57).gamma_multiply(k)
            }
            ClaudeStatus::Permission => Color32::from_rgb(0xe0, 0x4f, 0x5f),
            ClaudeStatus::Input => Color32::from_rgb(0xe5, 0xc0, 0x7b),
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
    fn exited(&self) -> bool {
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
    OpenPalette,
    OpenTemplates,
    OpenCosts,
    TemplateStart(usize),
    TemplateDelete(usize),
    TemplateSave(String, Option<String>),
    ResumeItem(history::HistoryItem),
    ToggleTree,
    FolderNew(String),
    FolderRename(String),
    FolderDelete(String),
    FolderToggle(String),
    FolderColor(String, Option<[u8; 3]>),
    FolderCwd(String, bool),
    FolderProfile(String, u64),
    FolderNewSession(String, bool),
    FolderRestartAll(String),
    FolderCloseAll(String),
    FolderLayout(String),
    FolderBroadcast(String),
    MoveFolder(String, String),
    FolderTemplate(String),
    ToggleComposer,
    SetProjectProfile(u64, u64),
    OpenQueue(u64),
    ToggleDiff,
    QueueAdd(u64, String),
    Approve(u64, bool),
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
    TagsPrompt(u64),
    ToggleFiles,
    InsertPath(String),
    NewSessionAt(String),
    OpenHistory,
    ResumeHistory(usize),
    OpenSearch,
    OpenSettings,
    OpenPrompts,
    OpenNotes(u64),
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
    Duplicate(u64),
    RequestClose(u64),
    Close(u64),
    Split(u64, Dir),
    SplitAt(u64, Dir, bool),
    HidePane(u64),
    Focus(u64),
    NewQuick,
}

struct NewDialog {
    shell_id: String,
    cwd: String,
    name: String,
    claude: bool,
    model: String,
    perm: usize,
    continue_last: bool,
    profile: u64,
    prompt: String,
    worktree: bool,
    branch: String,
    repo_for: String,
    repo: Option<PathBuf>,
    group: String,
}

const PERMS: [(&str, &str); 6] = [
    ("", "Standard"),
    ("acceptEdits", "acceptEdits"),
    ("plan", "plan"),
    ("auto", "auto"),
    ("dontAsk", "dontAsk"),
    ("bypassPermissions", "bypassPermissions"),
];

#[derive(Default)]
struct NewOpts {
    shell_id: Option<String>,
    cwd: Option<String>,
    name: Option<String>,
    claude: bool,
    claude_args: String,
    continue_last: bool,
    prompt: Option<String>,
    resume_id: Option<String>,
    transcript: String,
    /// None = Standardprofil des Projekts, Some(0) = kein Profil
    profile: Option<u64>,
    group: Option<String>,
    /// Sitzung nur anlegen, nicht im Fenster aktivieren (Agenten-Schnittstelle: kein Fokusklau).
    quiet: bool,
}

struct Launch<'a> {
    restore: bool,
    restart: bool,
    continue_last: bool,
    prompt: Option<&'a str>,
    resume_id: Option<&'a str>,
}

impl NewDialog {
    fn new(shell_id: String, claude: bool) -> Self {
        NewDialog { shell_id, cwd: home_dir(), name: String::new(), claude, model: String::new(), perm: 0, continue_last: false, profile: 0, prompt: String::new(), worktree: false, branch: String::new(), repo_for: String::new(), repo: None, group: String::new() }
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
}

struct TextPrompt {
    kind: PromptKind,
    id: u64,
    tags_for: Option<u64>,
    text: String,
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
    discard: bool,
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
    entries: BTreeMap<u64, Entry>,
    layout: Option<Node>,
    focused: u64,
    next_id: u64,
    want_focus: Option<u64>,
    rename: Option<(u64, String)>,
    group_prompt: Option<Prompt>,
    templates: Vec<Template>,
    templates_open: bool,
    tpl_name: String,
    cost_open: bool,
    budget_warned_day: String,
    last_budget_check: Instant,
    history_hits: Option<Vec<(history::HistoryItem, String)>>,
    history_hits_rx: Option<std::sync::mpsc::Receiver<Vec<(history::HistoryItem, String)>>>,
    scale_pending: Option<f32>,
    folder_prompt: Option<FolderPrompt>,
    folder_rects: Vec<(String, Rect)>,
    tree_geo: TreeGeo,
    new_dialog: Option<NewDialog>,
    confirm_close: Option<u64>,
    settings_open: bool,
    dir: PathBuf,
    last_save: Instant,
    applied_theme: String,
    last_title: String,
    mode: titlebar::Mode,
    db: Option<crate::db::Db>,
    db_error: Option<String>,
    tasks: tasks_ui::TasksUi,
    plan: planning_ui::PlanUi,
    /// Ticket↔Terminal-Verknüpfung (`terminal_links.rs`).
    tlinks: terminal_links::TLinks,
    backup_rx: Option<std::sync::mpsc::Receiver<Result<crate::backup::BackupInfo, String>>>,
    backup_checked: Instant,
    backup_msg: Option<String>,
    backups_cache: Option<Vec<crate::backup::BackupInfo>>,
    backups_dirty: bool,
    confirm_restore: Option<PathBuf>,
    restore_error: Option<String>,
    hook_server: Option<HookServer>,
    hooks_file: Option<PathBuf>,
    hooks_global: bool,
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
    detached: Vec<u64>,
    settings_tab: usize,
    capture_key: Option<String>,
    queue_open: Option<u64>,
    queue_draft: String,
    pending_actions: Vec<Action>,
    start_in_tray: bool,
    win_autostart: bool,
    win_context: bool,
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
    gsearch: Option<GlobalSearch>,
    notice: Option<(String, Instant)>,
    /// Toast-Stapel unten rechts (`toasts.rs`), Uhr dafür und Toast unter dem Mauszeiger.
    toasts: toasts::ToastQueue,
    toasts_epoch: Instant,
    toasts_hover: Option<u64>,
    hwnd: Option<isize>,
    in_tray: bool,
    editor: Option<Editor>,
    applied_font: String,
}

fn home_dir() -> String {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default()
}

fn setup_fonts(ctx: &egui::Context, choice: &str) {
    ctx.set_fonts(build_fonts(choice));
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
    // Optionale Systemschriften als Fallback für Symbole, Sonderzeichen, CJK, Thai, Indisch, Mathe usw.
    // Es werden nur vorhandene Dateien geladen; fehlt eine, ändert sich nichts.
    let candidates: [(&str, &str); 14] = [
        ("seguisym", r"C:\Windows\Fonts\seguisym.ttf"),
        ("segoeui", r"C:\Windows\Fonts\segoeui.ttf"),
        ("msyh", r"C:\Windows\Fonts\msyh.ttc"),
        ("msjh", r"C:\Windows\Fonts\msjh.ttc"),
        ("yugoth", r"C:\Windows\Fonts\YuGothM.ttc"),
        ("meiryo", r"C:\Windows\Fonts\meiryo.ttc"),
        ("malgun", r"C:\Windows\Fonts\malgun.ttf"),
        ("simsun", r"C:\Windows\Fonts\simsun.ttc"),
        ("nirmala", r"C:\Windows\Fonts\Nirmala.ttc"),
        ("leelawui", r"C:\Windows\Fonts\LeelawUI.ttf"),
        ("seguihis", r"C:\Windows\Fonts\seguihis.ttf"),
        ("cambria", r"C:\Windows\Fonts\cambria.ttc"),
        ("arial", r"C:\Windows\Fonts\arial.ttf"),
        ("dejavu", "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf"),
    ];
    for (name, path) in candidates {
        if let Ok(bytes) = std::fs::read(path) {
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
    if let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\segoeuib.ttf") {
        fonts.font_data.insert("segoeuib".to_owned(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
        bold.push("segoeuib".to_owned());
    }
    bold.extend(fonts.families.get(&egui::FontFamily::Proportional).cloned().unwrap_or_default());
    fonts.families.insert(egui::FontFamily::Name("bold".into()), bold);
    // Gewählte Terminalschrift an die erste Stelle setzen (Fallbacks bleiben dahinter).
    let key = match choice {
        "Hack" => Some("Hack".to_string()),
        "Consolas" | "Lucida Console" | "Courier New" => {
            let (k, f) = match choice {
                "Consolas" => ("consolas", "consola.ttf"),
                "Lucida Console" => ("lucon", "lucon.ttf"),
                _ => ("cour", "cour.ttf"),
            };
            std::fs::read(format!(r"C:\Windows\Fonts\{f}")).ok().map(|b| {
                fonts.font_data.insert(k.to_owned(), std::sync::Arc::new(egui::FontData::from_owned(b)));
                k.to_string()
            })
        }
        _ => None,
    };
    if let Some(k) = key {
        let fam = fonts.families.entry(egui::FontFamily::Monospace).or_default();
        fam.retain(|n| n != &k);
        fam.insert(0, k);
    }
    fonts
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Schrift wird in update() über apply_font gesetzt.
        let dir = state::data_dir();
        let mut settings: Settings = state::load_json(&dir.join("settings.json")).unwrap_or_default();
        settings.sanitize();
        // `--lang`: nur für diesen Start (beim Speichern bleibt die gespeicherte Sprache, siehe `state::ser_language`).
        if let Some(l) = crate::cli::get().lang {
            state::set_lang_override(l, settings.language);
            settings.language = l;
        }
        state::set_lang(settings.language);
        let (saved, startup_note) = state::load_saved(&dir.join("state.json"));
        // Vorgemerkte Wiederherstellung einspielen, dann die Datenbank öffnen (Migrationen inklusive).
        let restore_result = crate::backup::apply_pending_restore(&dir, crate::db::FILE);
        let backup_dir = crate::cli::get().backup_dir.clone().unwrap_or_else(|| crate::backup::dir(&dir, &settings.backup_dir));
        let (db, db_error) = match crate::db::open(&dir, &backup_dir) {
            Ok(d) => (Some(d), None),
            Err(e) => (None, Some(e.message(settings.language))),
        };
        let shells = shells::detect();
        let bg = std::sync::mpsc::channel::<String>();
        let mut app = App {
            settings,
            shells,
            themes: Vec::new(),
            entries: BTreeMap::new(),
            layout: None,
            focused: 0,
            next_id: saved.next_id.max(1),
            want_focus: None,
            rename: None,
            group_prompt: None,
            templates: templates::load(&dir),
            templates_open: false,
            tpl_name: String::new(),
            cost_open: false,
            budget_warned_day: String::new(),
            last_budget_check: Instant::now(),
            history_hits: None,
            history_hits_rx: None,
            scale_pending: None,
            folder_prompt: None,
            folder_rects: Vec::new(),
            tree_geo: TreeGeo::default(),
            new_dialog: None,
            confirm_close: None,
            settings_open: false,
            dir,
            last_save: Instant::now(),
            applied_theme: String::new(),
            last_title: String::new(),
            mode: titlebar::Mode::default(),
            db,
            db_error,
            tasks: tasks_ui::TasksUi::default(),
            plan: planning_ui::PlanUi::default(),
            backup_rx: None,
            backup_checked: Instant::now().checked_sub(Duration::from_secs(61)).unwrap_or_else(Instant::now),
            backup_msg: None,
            backups_cache: None,
            backups_dirty: true,
            confirm_restore: None,
            restore_error: None,
            hook_server: HookServer::start(&cc.egui_ctx),
            hooks_file: None,
            hooks_global: false,
            hooks_msg: String::new(),
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
            detached: saved.detached.clone(),
            settings_tab: 0,
            capture_key: None,
            queue_open: None,
            queue_draft: String::new(),
            pending_actions: Vec::new(),
            start_in_tray: crate::cli::get().tray,
            win_autostart: false,
            win_context: false,
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
            gsearch: None,
            notice: None,
            toasts: Default::default(),
            toasts_epoch: Instant::now(),
            toasts_hover: None,
            hwnd: crate::winctl::hwnd_from(cc),
            in_tray: false,
            editor: None,
            applied_font: String::new(),
        };
        // `--no-tray`: kein Tray-Symbol; Schließen/Minimieren verhalten sich dann normal (siehe `handle_tray`).
        if !crate::cli::get().no_tray {
            let (tx, rx) = std::sync::mpsc::channel();
            let l = app.settings.language;
            app.tray = Tray::create(&cc.egui_ctx, tx, l.t("Paneldeck anzeigen", "Show Paneldeck"), l.t("Beenden", "Quit"));
            app.tray_rx = rx;
        }
        if let Some(h) = app.hwnd {
            winchrome::init(h, &cc.egui_ctx); // runde Ecken + Snap-Layouts am Maximieren-Knopf
        }
        clip::init(&app.dir);
        app.reload_themes();
        app.prompts = state::load_json(&app.dir.join("prompts.json")).unwrap_or_else(|| prompts::defaults(app.settings.language));
        if let Some(h) = &app.hook_server {
            hooks::write_ipc_file(&app.dir, h.port, &h.ipc_token);
        }
        app.hooks_file = hooks::write_session_settings(&app.dir, &app.exe);
        app.hooks_global = hooks::global_settings_path().is_some_and(|p| hooks::global_installed(&p));
        if app.settings.default_shell.is_empty() {
            app.settings.default_shell = app.shells.first().map(|s| s.id.clone()).unwrap_or_default();
        }

        // Sitzungen wiederherstellen
        for meta in saved.sessions {
            app.next_id = app.next_id.max(meta.id.saturating_add(1));
            let id = meta.id;
            let mut entry = Entry::new(meta);
            app.spawn_into(&cc.egui_ctx, &mut entry, Launch { restore: true, restart: false, continue_last: false, prompt: None, resume_id: None });
            app.entries.insert(id, entry);
        }
        // Projekte: ältere Stände ohne Projekte bekommen ein Standardprojekt.
        if app.projects.is_empty() {
            app.projects.push(Project {
                id: 1,
                name: app.settings.language.t("Standard", "Default").to_string(),
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
        let p = app.projects.iter().find(|p| p.id == app.active_project).cloned().unwrap_or_default();
        app.load_project_view(p.layout, p.focused);
        app.detached.retain(|id| app.entries.contains_key(id));
        let det = app.detached.clone();
        for id in det {
            if app.layout.as_ref().is_some_and(|l| l.contains(id)) {
                app.hide_pane(id);
            }
        }
        if app.entries.is_empty() {
            app.new_session(&cc.egui_ctx, NewOpts::default());
        }
        let tr: Vec<(u64, String)> = app.entries.iter().map(|(i, e)| (*i, e.meta.transcript.clone())).collect();
        for (i, t) in tr {
            if !t.is_empty() {
                app.usage.set_path(i, PathBuf::from(t));
            }
        }
        // Start aus dem Explorer-Kontextmenü: Sitzung im übergebenen Ordner öffnen.
        let cli = crate::cli::get();
        if let Some(p) = &cli.open {
            app.open_folder(&cc.egui_ctx, p, cli.claude);
        }
        app.want_focus = Some(app.focused);
        if let Some(n) = startup_note {
            app.notify_text(n);
        }
        let lang = app.settings.language;
        if let Some(b) = state::SETTINGS_RESET.get() {
            app.notify_text(format!("{} {}", lang.t("Einstellungen zurückgesetzt (--reset-settings). Sicherung:", "Settings reset (--reset-settings). Backup:"), b.display()));
        }
        if let Some(r) = restore_result {
            app.notify_text(match r {
                Ok(p) => format!("{} {}", lang.t("Sicherung wiederhergestellt:", "Backup restored:"), p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()),
                Err(e) => format!("{}: {e}", lang.t("Wiederherstellung fehlgeschlagen", "Restore failed")),
            });
        }
        if let Some(e) = app.db_error.clone() {
            app.notify_text(e);
        }
        app
    }

    /// Neue Sitzung in einem Ordner (Dateien → deren Ordner).
    fn open_folder(&mut self, ctx: &egui::Context, path: &str, claude: bool) {
        let p = std::path::Path::new(path);
        let dir = if p.is_file() { p.parent().map(|d| d.to_string_lossy().to_string()).unwrap_or_default() } else { path.to_string() };
        self.new_session(ctx, NewOpts { cwd: Some(dir), claude: claude && self.settings.claude_mode, ..NewOpts::default() });
    }

    fn handle_ipc(&mut self, ctx: &egui::Context) {
        let cmds: Vec<IpcCmd> = self.hook_server.as_ref().map(|h| h.cmd_rx.try_iter().collect()).unwrap_or_default();
        for c in cmds {
            self.from_tray(ctx);
            if let IpcCmd::Open { path, claude } = c {
                self.open_folder(ctx, &path, claude);
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
        let fonts: Vec<&str> = font_files.iter().copied().filter(|f| std::path::Path::new(&format!(r"C:\Windows\Fonts\{f}")).exists()).collect();
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

    fn reload_themes(&mut self) {
        theme::write_example(&self.themes_dir());
        let mut t = theme::all();
        t.extend(theme::load_dir(&self.themes_dir()));
        self.themes = t;
    }

    fn binding(&self, id: &str) -> Option<(Modifiers, Key)> {
        self.settings.keys.get(id).and_then(|s| keys::parse(s)).or_else(|| {
            keys::DEFAULTS.iter().find(|(i, _)| *i == id).and_then(|(_, d)| keys::parse(d))
        })
    }

    /// Anzeigetext des aktuellen Kürzels (für Tooltips).
    fn binding_text(&self, id: &str) -> String {
        let s = self.settings.keys.get(id).cloned().or_else(|| keys::DEFAULTS.iter().find(|(i, _)| *i == id).map(|(_, d)| d.to_string())).unwrap_or_default();
        keys::pretty(&s, self.settings.language == Lang::De)
    }

    /// Layout/Fokus des aktiven Projekts übernehmen (nur gültige Sitzungen des Projekts).
    fn load_project_view(&mut self, layout: Option<Node>, focused: u64) {
        let pid = self.active_project;
        let ok = |id: &u64, s: &App| s.entries.get(id).is_some_and(|e| e.meta.project == pid);
        self.layout = layout.filter(|l| {
            let mut v = Vec::new();
            l.leaves(&mut v);
            !v.is_empty() && v.iter().all(|id| ok(id, self))
        });
        if self.layout.is_none() {
            if let Some(first) = self.docked_ids().first().copied() {
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
        self.entries.iter().filter(|(_, e)| e.meta.project == self.active_project).map(|(i, _)| *i).collect()
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
    }

    fn sidebar_match(&self, e: &Entry) -> bool {
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

    fn shell_by_id(&self, id: &str) -> Option<ShellSpec> {
        if let Some(pid) = id.strip_prefix("custom:").and_then(|n| n.parse::<u64>().ok()) {
            if let Some(p) = self.profile(pid) {
                let args = split_args(&p.args);
                // Eigenes wsl.exe-Programm: wie eine erkannte WSL-Shell behandeln (Pfadumrechnung, Hooks).
                // Andere bekannte Shells bekommen deren ID, damit eingefügte Pfade passend gequotet werden
                // (sonst bekäme ein cmd-/PowerShell-Profil bash-Quoting mit '…').
                let stem = std::path::Path::new(&p.program).file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
                let sid = match stem.as_str() {
                    "wsl" => format!("wsl:{}", shells::wsl_distro(&args).unwrap_or_default()),
                    "pwsh" | "powershell" | "cmd" => stem.clone(),
                    _ => id.to_string(),
                };
                return Some(ShellSpec { id: sid, label: p.name.clone(), program: p.program.clone(), args });
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

    fn claude_command(&self, meta: &mut SessionMeta, l: &Launch) -> String {
        let spec = self.shell_by_id(&meta.shell_id);
        let kind = spec.as_ref().map(|s| crate::quote::kind_of_program(&s.program)).unwrap_or(crate::quote::Kind::Posix);
        let sid = spec.as_ref().map(|s| s.id.clone()).unwrap_or_else(|| meta.shell_id.clone());
        let is_wsl = sid.starts_with("wsl");
        let mut c = String::from("claude");
        if is_wsl {
            // WSL: eigene Hook-Datei mit WSL-Pfad zur .exe (Aufruf über Interop); die globale Windows-Konfiguration gilt dort nicht.
            if let Some(f) = hooks::write_session_settings_wsl(&self.dir, &self.exe, &sid) {
                c += &format!(" --settings {}", crate::quote::quote(kind, &pathfmt::convert(&sid, &f.to_string_lossy())));
            }
        } else if !self.hooks_global {
            if let Some(f) = &self.hooks_file {
                c += &format!(" --settings {}", crate::quote::quote(kind, &f.to_string_lossy().replace('\\', "/")));
            }
        }
        // Gespeicherte Optionen nur in harmloser Form übernehmen (state.json ist eine Vertrauensgrenze).
        let args = crate::quote::sanitize_claude_args(&meta.claude_args);
        if !args.is_empty() {
            c += &format!(" {args}");
        }
        let resume_wanted = l.restart || (l.restore && self.settings.claude_resume);
        let resume_id = l.resume_id.filter(|id| crate::quote::is_uuid(id));
        if let Some(id) = resume_id {
            c += &format!(" --resume {id}");
        } else if resume_wanted && meta.claude_started && crate::quote::is_uuid(&meta.claude_session) {
            c += &format!(" --resume {}", meta.claude_session);
        } else if (resume_wanted && is_wsl) || l.continue_last {
            c += " --continue";
        } else {
            // Neue Konversation mit eigener ID, damit sie später exakt wiedergefunden wird.
            meta.claude_session = hooks::new_uuid();
            meta.claude_started = false;
            c += &format!(" --session-id {}", meta.claude_session);
        }
        // Der Start-Prompt wird nicht in die Kommandozeile geschrieben (Shell-Expansion von $, `, % …),
        // sondern als erster Eintrag der Warteschlange gesendet, sobald Claude bereit ist.
        if let Some(p) = l.prompt.filter(|p| !p.trim().is_empty()) {
            meta.queue.insert(0, p.trim().to_string());
        }
        c + "\r"
    }

    fn spawn_into(&self, ctx: &egui::Context, entry: &mut Entry, l: Launch) {
        let Some(shell) = self.shell_by_id(&entry.meta.shell_id) else {
            entry.error = Some(state::t("Keine Shell gefunden", "No shell found").into());
            return;
        };
        let restore_text = if l.restore {
            std::fs::read_to_string(self.scrollback_path(entry.meta.id)).ok()
        } else {
            None
        };
        let claude_cmd = if entry.meta.claude { Some(self.claude_command(&mut entry.meta, &l)) } else { None };
        let profile = self.profile(entry.meta.profile).cloned();
        let startup = profile.as_ref().map(|p| p.startup.trim().to_string()).filter(|s| !s.is_empty());
        let initial = match (&startup, claude_cmd) {
            (Some(st), Some(c)) => Some(format!("{st}\r{c}")),
            (Some(st), None) => Some(format!("{st}\r")),
            (None, c) => c,
        };
        let cwd = if entry.meta.cwd.is_empty() { home_dir() } else { entry.meta.cwd.clone() };
        let mut env = vec![(hooks::ENV_SESSION.to_string(), entry.meta.id.to_string())];
        if let Some(p) = &profile {
            for line in p.env.lines() {
                if let Some((k, v)) = line.split_once('=') {
                    if !k.trim().is_empty() {
                        env.push((k.trim().to_string(), v.trim().to_string()));
                    }
                }
            }
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
        if let Some(h) = &self.hook_server {
            env.push((hooks::ENV_PORT.to_string(), h.port.to_string()));
            env.push((hooks::ENV_TOKEN.to_string(), h.token.clone()));
            // Datenordner dieses Hubs: `paneldeck.exe mcp` aus diesem Terminal findet damit genau diese Instanz
            // (auch bei --data-dir/--profile). Kein Geheimnis – den IPC-Token liest die Brücke aus `ipc.json`.
            env.push((crate::paths::ENV_HUB_DATA.to_string(), self.dir.display().to_string()));
        }
        let wsl = shells::is_wsl(&shell);
        if wsl {
            // Variablen an WSL durchreichen (und zurück an die Windows-.exe beim Hook-Aufruf); dazu die Profilvariablen.
            let mut names = vec![hooks::ENV_SESSION.to_string(), hooks::ENV_PORT.to_string(), hooks::ENV_TOKEN.to_string()];
            for (k, _) in env.iter().skip(1) {
                if k != hooks::ENV_PORT && k != hooks::ENV_TOKEN && !names.contains(k) {
                    names.push(k.clone());
                }
            }
            let ours = names.join(":");
            let wslenv = match std::env::var("WSLENV") {
                Ok(v) if !v.is_empty() => format!("{v}:{ours}"),
                _ => ours,
            };
            env.push(("WSLENV".to_string(), wslenv));
        }
        entry.shell_eff = shell.id.clone();
        entry.status = ClaudeStatus::Unknown;
        let mut shell = shell;
        if self.settings.shell_integration {
            let (extra_args, extra_env) = shells::integration(&shell);
            shell.args.extend(extra_args);
            for (k, v) in extra_env {
                if wsl {
                    // Variable an WSL durchreichen.
                    if let Some(w) = env.iter_mut().find(|(n, _)| n == "WSLENV") {
                        w.1 = format!("{}:{k}", w.1);
                    }
                }
                env.push((k, v));
            }
        }
        match Session::spawn(
            ctx,
            SpawnOptions {
                shell: &shell,
                cwd: Some(&cwd),
                restore_text: restore_text.as_deref(),
                initial_input: initial.as_deref(),
                scrollback_lines: self.settings.scrollback_lines,
                env,
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
    fn new_session(&mut self, ctx: &egui::Context, o: NewOpts) -> u64 {
        let quiet = o.quiet;
        let id = self.next_id;
        self.next_id += 1;
        let pid = o.profile.unwrap_or_else(|| self.projects.iter().find(|p| p.id == self.active_project).map(|p| p.profile).unwrap_or(0));
        let prof = self.profile(pid).cloned();
        let shell_id = match &prof {
            Some(p) if !p.program.trim().is_empty() => format!("custom:{}", p.id),
            Some(p) if !p.shell_id.is_empty() && o.shell_id.is_none() => p.shell_id.clone(),
            _ => o.shell_id.clone().unwrap_or_else(|| self.settings.default_shell.clone()),
        };
        let o = NewOpts { cwd: o.cwd.filter(|c| !c.is_empty()).or_else(|| prof.as_ref().map(|p| p.cwd.clone()).filter(|c| !c.is_empty())), ..o };
        let label = self.shell_by_id(&shell_id).map(|s| s.label).unwrap_or_else(|| "Shell".into());
        let (name, custom) = match o.name.filter(|n| !n.trim().is_empty()) {
            Some(n) => (n, true),
            None if o.claude => ("Claude".to_string(), false),
            None => (label, false),
        };
        let meta = SessionMeta {
            id,
            name,
            custom_name: custom,
            shell_id,
            cwd: o.cwd.filter(|c| !c.is_empty()).or_else(|| self.projects.iter().find(|p| p.id == self.active_project).map(|p| p.folder.clone())).unwrap_or_default(),
            project: self.active_project,
            color: prof.as_ref().and_then(|p| p.color).unwrap_or(PALETTE[(id as usize) % PALETTE.len()]),
            profile: pid,
            claude: o.claude,
            claude_args: o.claude_args,
            claude_session: o.resume_id.clone().unwrap_or_default(),
            claude_started: o.resume_id.is_some(),
            transcript: o.transcript.clone(),
            group: o.group.as_deref().map(state::clean_folder_path).unwrap_or_default(),
            ..SessionMeta::default()
        };
        if !meta.group.is_empty() {
            let g = meta.group.clone();
            if let Some(pr) = self.projects.iter_mut().find(|p| p.id == meta.project) {
                pr.ensure_folder(&g);
            }
        }
        let mut entry = Entry::new(meta);
        self.spawn_into(
            ctx,
            &mut entry,
            Launch { restore: false, restart: false, continue_last: o.continue_last, prompt: o.prompt.as_deref(), resume_id: o.resume_id.as_deref() },
        );
        self.entries.insert(id, entry);
        if !quiet {
            self.activate(id);
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
        self.entries.iter().filter(|(_, e)| e.meta.project == self.active_project && !e.meta.group.is_empty() && state::folder_within(&e.meta.group, path)).map(|(i, _)| *i).collect()
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

    fn activate(&mut self, id: u64) {
        // Eine Sitzung darf nie gleichzeitig abgedockt und im Hauptlayout sein (zwei Größen kämpften um die PTY).
        self.detached.retain(|d| *d != id);
        match &mut self.layout {
            None => self.layout = Some(Node::Leaf(id)),
            Some(l) => {
                if !l.contains(id) {
                    l.replace_leaf(self.focused, id);
                    if !l.contains(id) {
                        // fokussierter Bereich unbekannt → erstes Blatt ersetzen
                        let mut v = Vec::new();
                        l.leaves(&mut v);
                        if let Some(first) = v.first() {
                            l.replace_leaf(*first, id);
                        }
                    }
                }
            }
        }
        self.focused = id;
        self.want_focus = Some(id);
        if let Some(e) = self.entries.get_mut(&id) {
            e.attention = false;
        }
    }

    fn split_at(&mut self, ctx: &egui::Context, target: u64, dir: Dir, before: bool) {
        let Some(src) = self.entries.get(&target) else { return };
        let (shell, cwd) = (src.meta.shell_id.clone(), src.cwd_now());
        let id = self.next_id;
        self.next_id += 1;
        let label = self.shell_by_id(&shell).map(|s| s.label).unwrap_or_else(|| "Shell".into());
        let meta = SessionMeta {
            id,
            name: label,
            custom_name: false,
            shell_id: shell,
            cwd,
            color: PALETTE[(id as usize) % PALETTE.len()],
            group: self.entries[&target].meta.group.clone(),
            project: self.active_project,
            profile: self.entries[&target].meta.profile,
            ..SessionMeta::default()
        };
        let mut entry = Entry::new(meta);
        self.spawn_into(ctx, &mut entry, Launch { restore: false, restart: false, continue_last: false, prompt: None, resume_id: None });
        self.entries.insert(id, entry);
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
            if let Some(l) = &self.layout {
                let mut v = Vec::new();
                l.leaves(&mut v);
                self.focused = v.first().copied().unwrap_or(0);
                self.want_focus = Some(self.focused);
            }
        }
    }

    fn close_session(&mut self, id: u64) {
        self.hide_pane(id);
        self.detached.retain(|d| *d != id);
        self.entries.remove(&id);
        self.usage.remove(id);
        self.git.remove(id);
        let _ = std::fs::remove_file(self.scrollback_path(id));
        if self.layout.is_none() {
            if let Some(first) = self.docked_ids().first().copied() {
                self.activate(first);
            }
        }
    }

    fn cycle(&mut self, delta: i32) {
        let ids: Vec<u64> = self.docked_ids();
        if ids.is_empty() {
            return;
        }
        let pos = ids.iter().position(|i| *i == self.focused).unwrap_or(0) as i32;
        let next = (pos + delta).rem_euclid(ids.len() as i32) as usize;
        self.activate(ids[next]);
    }

    fn save_all(&self) {
        state::save_json(&self.dir.join("settings.json"), &self.settings);
        state::save_json(&self.dir.join("prompts.json"), &self.prompts);
        let _ = std::fs::create_dir_all(self.dir.join("scrollback"));
        let saved = SavedState {
            sessions: self.entries.values().map(|e| e.meta.clone()).collect(),
            layout: self.layout.clone(),
            focused: self.focused,
            next_id: self.next_id,
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
        };
        state::save_json(&self.dir.join("state.json"), &saved);
        for (id, e) in &self.entries {
            if let Some(s) = &e.live {
                let _ = state::write_atomic(&self.scrollback_path(*id), s.dump_text(3000).as_bytes());
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
        let a = (self.settings.opacity.clamp(0.3, 1.0) * 255.0).round() as u8;
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
        ] {
            if let Some((m, k)) = self.binding(id) {
                if ctx.input_mut(|i| consume_key_physical(i, m, k)) {
                    self.set_mode(mode);
                }
            }
        }
    }

    /// Bereich wechseln; beim Zurück in die Terminals bekommt das zuletzt aktive Terminal wieder den Fokus.
    fn set_mode(&mut self, mode: titlebar::Mode) {
        if self.mode != mode && mode == titlebar::Mode::Terminals {
            self.want_focus = Some(self.focused);
        }
        if mode != titlebar::Mode::Tasks {
            self.plan.forget_terminal_return(); // von Hand weggeschaltet: der Rückweg „Zurück zum Terminal Hub“ gilt nicht mehr
        }
        self.mode = mode;
    }

    fn shortcuts(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        if self.capture_key.is_some() {
            return; // gerade wird ein Kürzel aufgenommen
        }
        let mut binds: Vec<(&'static str, Modifiers, Key)> =
            keys::DEFAULTS.iter().filter(|(id, _)| !keys::is_global(id)).filter_map(|(id, _)| self.binding(id).map(|(m, k)| (*id, m, k))).collect();
        // Spezifischere Kürzel (mehr Modifikatoren) zuerst prüfen.
        binds.sort_by_key(|b| std::cmp::Reverse(b.1.ctrl as u8 + b.1.shift as u8 + b.1.alt as u8));
        let mut fired: Vec<&'static str> = Vec::new();
        let mut extra: Vec<Action> = Vec::new();
        let mut alt_n: Option<usize> = None;
        let cs = Modifiers::CTRL | Modifiers::SHIFT;
        ctx.input_mut(|i| {
            for (id, m, k) in &binds {
                if i.consume_key(*m, *k) {
                    fired.push(id);
                }
            }
            for (n, k) in [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9].into_iter().enumerate() {
                if i.consume_key(cs, k) {
                    extra.push(Action::SwitchProject(n as u64 + 1_000_000));
                }
                if i.consume_key(Modifiers::ALT, k) {
                    alt_n = Some(n);
                }
            }
            let ca = Modifiers::CTRL | Modifiers::ALT;
            for (k, n) in [(Key::ArrowLeft, Nav::Left), (Key::ArrowRight, Nav::Right), (Key::ArrowUp, Nav::Up), (Key::ArrowDown, Nav::Down)] {
                if i.consume_key(ca, k) {
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
                "hide" => Action::HidePane(f),
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
        let theme = self.theme();
        let font = FontId::monospace(self.settings.font_size);
        let lang = self.settings.language;

        // Broadcast: nur die fokussierte Sitzung zeichnet Eingaben mit.
        for (id, e) in &self.entries {
            if let Some(s) = &e.live {
                s.set_tee(self.broadcast && *id == self.focused);
            }
        }
        {
            let n = self.layout.as_ref().map(|l| { let mut v = Vec::new(); l.leaves(&mut v); v.len() }).unwrap_or(0);
            egui::TopBottomPanel::top("bcbar")
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
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(theme.bg))
            .show(ctx, |ui| {
                let rect = ui.available_rect_before_wrap();
                let Some(mut layout) = self.layout.take() else {
                    let first_run = self.entries.is_empty();
                    let claude_mode = self.settings.claude_mode;
                    ui.scope_builder(UiBuilder::new().max_rect(Rect::from_center_size(rect.center(), Vec2::new(420.0, 300.0))), |ui| {
                        ui.vertical_centered(|ui| {
                            let (r, _) = ui.allocate_exact_size(Vec2::splat(56.0), Sense::hover());
                            ui.painter().rect_filled(r, 14.0, style::ACCENT.gamma_multiply(0.18));
                            style::paint_icon(ui.painter(), SIcon::Spark, r.center(), style::ACCENT);
                            ui.add_space(S3);
                            ui.heading(if first_run { lang.t("Willkommen bei Paneldeck", "Welcome to Paneldeck") } else { lang.t("Keine Sitzung in diesem Projekt", "No session in this project") });
                            ui.add_space(S1);
                            ui.label(RichText::new(lang.t("Starte ein Terminal oder eine Claude-Sitzung – alles an einem Ort.", "Start a terminal or a Claude session – all in one place.")).color(style::dim(ui)));
                            ui.add_space(S3);
                            ui.horizontal(|ui| {
                                ui.add_space((ui.available_width() - if claude_mode { 300.0 } else { 150.0 }) / 2.0);
                                if ui.add(egui::Button::new(RichText::new(lang.t("Neue Sitzung", "New session")).strong()).fill(style::ACCENT.gamma_multiply(0.85)).min_size(Vec2::new(140.0, 32.0))).clicked() {
                                    actions.push(Action::NewQuick);
                                }
                                if claude_mode && ui.add(egui::Button::new(lang.t("Claude starten …", "Start Claude …")).min_size(Vec2::new(140.0, 32.0))).clicked() {
                                    actions.push(Action::NewDialog(true));
                                }
                            });
                            ui.add_space(S3 * 1.5);
                            let hint = |ui: &mut Ui, keys: &str, what: &str| {
                                ui.horizontal(|ui| {
                                    ui.add_space((ui.available_width() - 330.0).max(0.0) / 2.0);
                                    ui.label(RichText::new(keys).monospace().color(style::dim(ui)));
                                    ui.label(RichText::new(what).color(style::dim(ui)));
                                });
                            };
                            hint(ui, "Strg+Umschalt+P", lang.t("Befehle & Suche", "Commands & search"));
                            hint(ui, "Strg+Umschalt+T", lang.t("Neue Sitzung", "New session"));
                            hint(ui, "Strg+Umschalt+D/E", lang.t("Bereich teilen", "Split pane"));
                        });
                    });
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
                };
                show_node(ui, &mut layout, rect, &mut cx);
                self.layout = Some(layout);
            });

        // Broadcast weiterleiten
        if self.broadcast {
            let data = self.entries.get(&self.focused).and_then(|e| e.live.as_ref()).map(|s| s.take_tee()).unwrap_or_default();
            if !data.is_empty() {
                let mut leaves = Vec::new();
                if let Some(l) = &self.layout {
                    l.leaves(&mut leaves);
                }
                for id in leaves.into_iter().filter(|i| *i != self.focused) {
                    if let Some(s) = self.entries.get(&id).and_then(|e| e.live.as_ref()) {
                        s.write(&data);
                    }
                }
            }
        }
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
        let mut fallback_done: Vec<String> = Vec::new();
        for (id, e) in self.entries.iter_mut() {
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
            if e.meta.claude && !e.hook_seen && !e.exited() {
                if let Some(u) = self.usage.get(*id) {
                    // Großzügige Frist: Claude schreibt bei langen Werkzeugläufen (Tests, Builds) lange nichts ins Transkript;
                    // ein zu früh erkanntes „bereit“ würde den nächsten Warteschlangen-Prompt mitten in die Arbeit tippen.
                    let recent = u.modified.and_then(|m| m.elapsed().ok()).is_some_and(|d| d < Duration::from_secs(20));
                    let old = e.status;
                    e.status = if recent { ClaudeStatus::Working } else { ClaudeStatus::Idle };
                    if e.queue_tick(old, e.status) {
                        fallback_done.push(e.display_name());
                    }
                }
            }
        }
        self.queue_finished(ctx, fallback_done);
    }


    fn project_bar(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let lang = self.settings.language;
        let projects: Vec<(u64, String, [u8; 3], usize)> = self
            .projects
            .iter()
            .map(|p| (p.id, p.name.clone(), p.color, self.entries.values().filter(|e| e.meta.project == p.id).count()))
            .collect();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(4.0, 4.0);
            for (id, name, c, n) in projects {
                let col = Color32::from_rgb(c[0], c[1], c[2]);
                let r = style::pill(ui, &name, Some(n), col, id == self.active_project);
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
                    ui.menu_button(lang.t("Farbe", "Color"), |ui| {
                        ui.horizontal(|ui| {
                            for c in PALETTE {
                                let (r, resp) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
                                ui.painter().rect_filled(r, 4.0, Color32::from_rgb(c[0], c[1], c[2]));
                                if resp.clicked() {
                                    actions.push(Action::SetProjectColor(id, c));
                                    ui.close();
                                }
                            }
                        });
                    });
                    if self.projects.len() > 1 {
                        ui.separator();
                        let confirm_id = egui::Id::new(("delproj", id));
                        let now_t = ui.input(|i| i.time);
                        let armed = ui.memory(|m| m.data.get_temp::<f64>(confirm_id)).is_some_and(|t| now_t - t < 4.0);
                        let label = if armed {
                            lang.t("Wirklich löschen? Nochmal klicken", "Really delete? Click again")
                        } else {
                            lang.t("Projekt löschen (Sitzungen bleiben)", "Delete project (sessions are kept)")
                        };
                        if ui.button(label).clicked() {
                            if armed {
                                ui.memory_mut(|m| m.data.remove::<f64>(confirm_id));
                                actions.push(Action::DeleteProject(id));
                                ui.close();
                            } else {
                                ui.memory_mut(|m| m.data.insert_temp(confirm_id, now_t));
                            }
                        }
                    }
                });
            }
            if style::icon_btn_small(ui, SIcon::Plus, lang.t("Neues Projekt", "New project")).clicked() {
                actions.push(Action::NewProjectPrompt);
            }
        });
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
                    let id = self.new_session(ctx, p.opts);
                    if let Some(e) = self.entries.get_mut(&id) {
                        e.meta.worktree = Some(p.wt);
                    }
                }
                Err(e) => self.notify_text(format!("{}: {e}", self.settings.language.t("Worktree konnte nicht angelegt werden", "Could not create worktree"))),
            }
        }
    }

    fn notify_text(&mut self, text: String) {
        self.notice = Some((text, Instant::now()));
    }

    /// Ordner (OSC) und beendete Befehle der Shell-Integration auswerten.
    fn poll_shell(&mut self, ctx: &egui::Context) {
        let window_focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        let threshold = self.settings.cmd_notify_secs as u64 * 1000;
        let lang = self.settings.language;
        let focused_id = self.focused;
        let mut toasts: Vec<(String, CmdDone)> = Vec::new();
        for (id, e) in self.entries.iter_mut() {
            let Some(s) = e.live.as_ref() else { continue };
            let (cwd, done) = {
                let mut st = s.shell_state.lock().unwrap_or_else(|e| e.into_inner());
                (st.cwd.take(), std::mem::take(&mut st.done))
            };
            if let Some(c) = cwd {
                e.live_cwd = pathfmt::from_wsl(e.eff_shell(), &c);
            }
            for d in done {
                if threshold > 0 && d.dur_ms.unwrap_or(0) >= threshold && (!window_focused || *id != focused_id) {
                    e.attention = true;
                    toasts.push((e.display_name(), d));
                }
            }
        }
        for (name, d) in toasts {
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
                    ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
                }
            } else {
                self.notify_text(format!("{name}: {body}"));
            }
        }
    }

    /// Gesamten Verlauf einer Sitzung als Textdatei speichern.
    fn export_history(&mut self, id: u64) {
        let lang = self.settings.language;
        let Some(e) = self.entries.get(&id) else { return };
        let Some(s) = e.live.as_ref() else { return };
        let text = s.dump_text(2_000_000);
        let safe: String = e.display_name().chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let name = format!("{safe}-{secs}.txt");
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

    fn poll_history(&mut self) {
        if let Some(rx) = &self.history_hits_rx {
            if let Ok(h) = rx.try_recv() {
                self.history_hits = Some(h);
                self.history_hits_rx = None;
            }
        }
        if let Some(rx) = &self.history_rx {
            if let Ok(items) = rx.try_recv() {
                self.history = items;
                self.history_rx = None;
            }
        }
    }

    fn poll_hooks(&mut self, ctx: &egui::Context) {
        let window_focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        // Ereignisse in der angesehenen Sitzung gelten als gesehen.
        if window_focused {
            if let Some(e) = self.entries.get_mut(&self.focused) {
                e.attention = false;
            }
        }
        let msgs: Vec<HookMsg> = self.hook_server.as_ref().map(|s| s.rx.try_iter().collect()).unwrap_or_default();
        let lang = self.settings.language;
        let mut queue_done: Vec<String> = Vec::new();
        for m in msgs {
            let focused_id = self.focused;
            let Some(e) = self.entries.get_mut(&m.session) else { continue };
            let old = e.status;
            e.hook_seen = true;
            let sh = e.eff_shell().to_string();
            let m = HookMsg {
                transcript_path: m.transcript_path.as_ref().map(|p| pathfmt::from_wsl(&sh, p)),
                cwd: m.cwd.as_ref().map(|p| pathfmt::from_wsl(&sh, p)),
                ..m
            };
            if let Some(tp) = m.transcript_path.as_ref().filter(|p| !p.is_empty()) {
                if e.meta.transcript != *tp {
                    e.meta.transcript = tp.clone();
                    self.usage.set_path(m.session, PathBuf::from(tp));
                }
            }
            if let Some(c) = m.cwd.as_ref().filter(|c| !c.is_empty()) {
                e.live_cwd = c.clone();
            }
            if e.meta.claude {
                if let Some(cs) = m.claude_session.as_ref().filter(|c| !c.is_empty()) {
                    e.meta.claude_session = cs.clone();
                }
                if m.event == "UserPromptSubmit" {
                    e.meta.claude_started = true;
                }
            }
            let new = hooks::status_after(old, &m);
            e.status = new;
            if new == ClaudeStatus::Permission && old != ClaudeStatus::Permission {
                e.perm_since = Some(std::time::Instant::now());
            }
            if e.queue_tick(old, new) {
                queue_done.push(e.display_name());
            }
            if new != old {
                e.status_msg = m.message.clone().unwrap_or_default();
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
                    ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
                }
            }
        }
        self.queue_finished(ctx, queue_done);
    }

    fn queue_finished(&mut self, ctx: &egui::Context, names: Vec<String>) {
        let lang = self.settings.language;
        let window_focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        for name in names {
            let body = lang.t("Warteschlange abgearbeitet", "Queue finished");
            if window_focused {
                self.notify_text(format!("{name}: {body}"));
            } else {
                if self.settings.notify_toast {
                    notify::toast(&name, body, self.settings.notify_sound);
                }
                if self.settings.notify_flash {
                    ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
                }
            }
        }
    }

    fn apply(&mut self, ctx: &egui::Context, a: Action) {
        match a {
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
            Action::OpenQueue(id) => {
                self.queue_open = if self.queue_open == Some(id) { None } else { Some(id) };
            }
            Action::QueueAdd(id, text) => {
                let text = text.trim().to_string();
                if !text.is_empty() {
                    if let Some(e) = self.entries.get_mut(&id) {
                        e.meta.queue.push(text);
                        // Ist Claude schon bereit, startet die Warteschlange sofort.
                        if e.meta.claude && !e.meta.queue_paused && e.status == ClaudeStatus::Idle && !e.exited() {
                            e.send_next_queued();
                        }
                    }
                }
            }
            Action::Approve(id, yes) => {
                if let Some(e) = self.entries.get_mut(&id) {
                    let settled = e.perm_since.is_none_or(|t| t.elapsed() >= std::time::Duration::from_millis(500));
                    if e.status == ClaudeStatus::Permission && settled && !e.exited() {
                        if let Some(s) = &e.live {
                            s.write(if yes { b"\r" } else { b"\x1b" });
                        }
                        e.status = if yes { ClaudeStatus::Working } else { ClaudeStatus::Idle };
                        e.status_msg.clear();
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
                    let matches = search::find_all(&s.term.lock().unwrap_or_else(|e| e.into_inner()), &query);
                    let mut st = SearchState::new();
                    st.focus_input = false;
                    st.current = matches.iter().position(|m| m.line == line).unwrap_or(0);
                    st.query = query.clone();
                    st.last_query = query;
                    st.matches = matches;
                    search::scroll_to(&mut s.term.lock().unwrap_or_else(|e| e.into_inner()), line);
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
                if let Some(s) = self.entries.get(&self.focused).and_then(|e| e.live.as_ref()) {
                    s.send_text(&format!("{} ", pathfmt::for_shell(&s.shell_id, &p)), false);
                }
                self.want_focus = Some(self.focused);
            }
            Action::NewSessionAt(dir) => {
                self.new_session(ctx, NewOpts { cwd: Some(dir), ..NewOpts::default() });
            }
            Action::OpenPalette => self.palette = Some(Palette { query: String::new(), sel: 0 }),
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
                {
                    let name: String = h.title.chars().take(32).collect();
                    self.new_session(
                        ctx,
                        NewOpts {
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
            Action::OpenPrompts => self.prompts_open = true,
            Action::NewDialog(claude) => {
                self.new_dialog = Some(NewDialog::new(self.settings.default_shell.clone(), claude && self.settings.claude_mode));
            }
            Action::OpenNotes(id) => {
                self.notes_open = if self.notes_open == Some(id) { None } else { Some(id) };
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
                if let (Some(sn), Some(s)) = (self.prompts.get(i), self.entries.get(&self.focused).and_then(|e| e.live.as_ref())) {
                    s.send_text(&sn.text, sn.submit);
                }
                self.want_focus = Some(self.focused);
            }
            Action::FocusDir(nav) => {
                if let Some(cur) = self.pane_rects.get(&self.focused).copied() {
                    let c = cur.center();
                    let best = self
                        .pane_rects
                        .iter()
                        .filter(|(id, _)| **id != self.focused)
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
                        .min_by(|a, b| a.1.total_cmp(&b.1));
                    if let Some((id, _)) = best {
                        self.activate(id);
                    }
                }
            }
            Action::Detach(id) => {
                if self.entries.contains_key(&id) && !self.detached.contains(&id) {
                    self.hide_pane(id);
                    self.detached.push(id);
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
            Action::NewProjectPrompt => self.text_prompt = Some(TextPrompt { kind: PromptKind::NewProject, id: 0, tags_for: None, text: String::new() }),
            Action::ProjectPrompt(id, kind) => {
                let text = self
                    .projects
                    .iter()
                    .find(|p| p.id == id)
                    .map(|p| if kind == PromptKind::ProjectFolder { p.folder.clone() } else { p.name.clone() })
                    .unwrap_or_default();
                self.text_prompt = Some(TextPrompt { kind, id, tags_for: None, text });
            }
            Action::SetProjectColor(id, c) => {
                if let Some(p) = self.projects.iter_mut().find(|p| p.id == id) {
                    p.color = c;
                }
            }
            Action::TagsPrompt(id) => {
                let text = self.entries.get(&id).map(|e| e.meta.tags.join(", ")).unwrap_or_default();
                self.text_prompt = Some(TextPrompt { kind: PromptKind::NewProject, id: 0, tags_for: Some(id), text });
            }
            Action::DeleteProject(id) => {
                let other = self.projects.iter().find(|p| p.id != id).map(|p| p.id);
                if let (true, Some(other)) = (self.projects.len() > 1, other) {
                    for e in self.entries.values_mut() {
                        if e.meta.project == id {
                            e.meta.project = other;
                        }
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
                if let Some(pid) = self.entries.get(&id).map(|e| e.meta.project) {
                    self.switch_project(pid);
                }
                self.activate(id)
            }
            Action::StartRename(id) => {
                if let Some(e) = self.entries.get(&id) {
                    self.rename = Some((id, e.display_name()));
                }
            }
            Action::SetColor(id, c) => {
                if let Some(e) = self.entries.get_mut(&id) {
                    e.meta.color = c;
                }
            }
            Action::SetGroup(id, g) => self.set_group(id, g),
            Action::OpenTemplates => self.templates_open = true,
            Action::OpenCosts => self.cost_open = true,
            Action::TemplateDelete(i) => {
                if i < self.templates.len() {
                    self.templates.remove(i);
                    templates::save(&self.dir, &self.templates);
                }
            }
            Action::TemplateSave(name, folder) => {
                let pid = self.active_project;
                let mut ids: Vec<u64> = self
                    .entries
                    .iter()
                    .filter(|(_, e)| e.meta.project == pid && folder.as_ref().is_none_or(|f| !e.meta.group.is_empty() && state::folder_within(&e.meta.group, f)))
                    .map(|(i, _)| *i)
                    .collect();
                ids.sort();
                let items: Vec<TplItem> = ids
                    .iter()
                    .filter_map(|i| self.entries.get(i))
                    .map(|e| TplItem {
                        name: e.display_name(),
                        shell_id: e.meta.shell_id.clone(),
                        cwd: e.cwd_now(),
                        claude: e.meta.claude,
                        claude_args: e.meta.claude_args.clone(),
                        profile: e.meta.profile,
                        group: e.meta.group.clone(),
                        prompt: String::new(),
                    })
                    .collect();
                let lang = self.settings.language;
                if items.is_empty() {
                    self.notify_text(lang.t("Keine Sitzungen zum Speichern", "No sessions to save").to_string());
                } else {
                    let n = items.len();
                    let name = if name.trim().is_empty() { format!("{} {}", lang.t("Vorlage", "Template"), self.templates.len() + 1) } else { name.trim().to_string() };
                    self.templates.retain(|t| t.name != name);
                    self.templates.push(Template { name: name.clone(), grid: n > 1, items });
                    templates::save(&self.dir, &self.templates);
                    self.notify_text(format!("{} {} ({} {})", lang.t("Vorlage gespeichert:", "Template saved:"), lang.q(&name), n, lang.t("Sitzungen", "sessions")));
                }
            }
            Action::TemplateStart(i) => {
                if let Some(t) = self.templates.get(i).cloned() {
                    let mut ids = Vec::new();
                    for it in t.items.iter().take(24) {
                        let id = self.new_session(
                            ctx,
                            NewOpts {
                                shell_id: Some(it.shell_id.clone()).filter(|x| !x.is_empty()),
                                cwd: Some(it.cwd.clone()),
                                name: Some(it.name.clone()),
                                claude: it.claude && self.settings.claude_mode,
                                claude_args: crate::quote::sanitize_claude_args(&it.claude_args),
                                prompt: Some(it.prompt.clone()).filter(|x| !x.trim().is_empty()),
                                profile: Some(it.profile),
                                group: Some(it.group.clone()),
                                ..NewOpts::default()
                            },
                        );
                        ids.push(id);
                    }
                    if t.grid && ids.len() > 1 {
                        let shown: Vec<u64> = ids.iter().copied().take(12).collect();
                        if let Some(l) = Node::grid(&shown, Dir::Horizontal) {
                            self.layout = Some(l);
                            self.focused = shown[0];
                            self.want_focus = Some(shown[0]);
                        }
                    }
                    self.templates_open = false;
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
            Action::FolderBroadcast(path) => self.folder_prompt = Some(FolderPrompt { kind: 2, path, text: String::new() }),
            Action::FolderDelete(path) => {
                let pid = self.active_project;
                for e in self.entries.values_mut() {
                    if e.meta.project == pid && state::folder_within(&e.meta.group, &path) {
                        e.meta.group = state::lift_path(&e.meta.group, &path);
                    }
                }
                if let Some(p) = self.projects.iter_mut().find(|p| p.id == pid) {
                    p.delete_folder_lift(&path);
                }
            }
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
                let mut d = NewDialog::new(self.settings.default_shell.clone(), claude && self.settings.claude_mode);
                if let Some(c) = cwd.or_else(|| proj.map(|p| p.folder.clone()).filter(|c| !c.is_empty())) {
                    d.cwd = c;
                }
                if let Some(pr) = prof {
                    d.profile = pr;
                }
                d.group = path;
                self.new_dialog = Some(d);
            }
            Action::FolderRestartAll(path) => {
                for id in self.folder_sessions(&path) {
                    if self.entries.get(&id).is_some_and(|e| e.exited()) {
                        self.apply(ctx, Action::Restart(id));
                    }
                }
            }
            Action::FolderCloseAll(path) => {
                for id in self.folder_sessions(&path) {
                    self.close_session(id);
                }
            }
            Action::FolderLayout(path) => {
                let ids: Vec<u64> = self.folder_sessions(&path).into_iter().filter(|id| self.entries.get(id).is_some_and(|e| !e.exited())).take(12).collect();
                if let Some(l) = Node::grid(&ids, Dir::Horizontal) {
                    self.detached.retain(|d| !ids.contains(d));
                    self.layout = Some(l);
                    self.focused = ids[0];
                    self.want_focus = Some(ids[0]);
                }
            }
            Action::Restart(id) => {
                if let Some(mut e) = self.entries.remove(&id) {
                    e.live = None;
                    self.spawn_into(ctx, &mut e, Launch { restore: false, restart: true, continue_last: false, prompt: None, resume_id: None });
                    self.entries.insert(id, e);
                }
            }
            Action::Duplicate(id) => {
                if let Some(src) = self.entries.get(&id) {
                    let o = NewOpts {
                        shell_id: Some(src.meta.shell_id.clone()),
                        cwd: Some(src.meta.cwd.clone()),
                        name: Some(format!("{} (2)", src.display_name())),
                        claude: src.meta.claude,
                        claude_args: src.meta.claude_args.clone(),
                        profile: Some(src.meta.profile),
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
                let changes = self
                    .entries
                    .get(&id)
                    .and_then(|e| e.meta.worktree.as_ref())
                    .map(|w| gitops::status(std::path::Path::new(&w.path)).map(|c| c.len()).unwrap_or(0))
                    .unwrap_or(0);
                self.wt_close = Some(WtClose { id, changes, discard: false });
            }
            Action::RequestClose(id) => {
                let running = self.entries.get(&id).is_some_and(|e| !e.exited());
                if running && self.settings.confirm_close {
                    self.confirm_close = Some(id);
                } else {
                    self.close_session(id);
                }
            }
            Action::Close(id) => self.close_session(id),
            Action::Split(id, dir) => self.split_at(ctx, id, dir, false),
            Action::SplitAt(id, dir, before) => self.split_at(ctx, id, dir, before),
            Action::HidePane(id) => {
                // Das letzte sichtbare Terminal bleibt stehen.
                let n = self.layout.as_ref().map(|l| { let mut v = Vec::new(); l.leaves(&mut v); v.len() }).unwrap_or(0);
                if n > 1 {
                    self.hide_pane(id);
                }
            }
            Action::NewQuick => {
                self.new_session(ctx, NewOpts::default());
            }
        }
    }
}

/// Wie `consume_key`, erkennt aber auch die physische Taste: mit Umschalt liefert die Tastatur eine andere logische Taste
/// (Umschalt+1 = „!“), `consume_key` sähe dann nicht `Key::Num1`.
fn consume_key_physical(i: &mut egui::InputState, m: Modifiers, k: Key) -> bool {
    let pos = i.events.iter().position(|e| {
        matches!(e, Event::Key { key, physical_key, pressed: true, modifiers, .. }
            if (*key == k || *physical_key == Some(k)) && modifiers.matches_logically(m))
    });
    match pos {
        Some(p) => {
            i.events.remove(p);
            true
        }
        None => false,
    }
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::TRANSPARENT.to_array()
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.sync_accent();
        let theme = self.theme();
        let key = format!("{}|{}|{}|{:?}", theme.name, theme.bg.a(), self.settings.animations, style::accent());
        if self.applied_theme != key {
            style::set_animations(self.settings.animations);
            style::apply(ctx, &theme, self.settings.opacity);
            self.applied_theme = key;
        }
        self.apply_font(ctx);
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
        if (ctx.zoom_factor() - self.settings.ui_scale).abs() > 0.001 {
            ctx.set_zoom_factor(self.settings.ui_scale);
        }
        #[cfg(feature = "readme-shots")]
        self.dev_shot_mode();
        self.handle_ipc(ctx);
        self.poll_mcp(ctx);
        crate::usage::set_prices(self.settings.prices.clone());
        self.handle_tray(ctx);

        self.poll_hooks(ctx);
        self.poll_shell(ctx);
        self.poll_background(ctx);
        self.sync_watchers(ctx);
        self.tlinks_frame(ctx);
        self.poll_history();
        self.check_budget(ctx);
        self.handle_drops(ctx);
        let mut actions: Vec<Action> = Vec::new();
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
        } else {
            self.tasks_view(ctx);
        }
        if let Some((text, t0)) = &self.notice {
            if t0.elapsed() > Duration::from_secs(5) {
                self.notice = None;
            } else {
                let text = text.clone();
                egui::Area::new(Id::new("notice")).anchor(Align2::CENTER_BOTTOM, Vec2::new(0.0, -18.0)).order(egui::Order::Tooltip).show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).corner_radius(CornerRadius::same(10)).show(ui, |ui| {
                        ui.label(text);
                    });
                });
                ctx.request_repaint_after(Duration::from_millis(500));
            }
        }
        self.toasts_ui(ctx);
        self.detached_windows(ctx, &mut actions);
        self.text_prompt_ui(ctx, &mut actions);
        self.dialogs(ctx, &mut actions);
        self.tlinks_picker_ui(ctx);
        self.restore_dialog(ctx);
        for a in actions {
            self.apply(ctx, a);
        }

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

        if self.last_save.elapsed() > Duration::from_secs(60) {
            self.save_all();
            self.last_save = Instant::now();
        }
        let animating = self.entries.values().any(|e| e.status == ClaudeStatus::Working && !e.exited());
        ctx.request_repaint_after(Duration::from_millis(if animating { 80 } else { 500 }));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        winchrome::shutdown();
        self.save_all();
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
}


const GAP: f32 = 6.0;
const HEADER_H: f32 = 30.0;

fn mix_col(a: Color32, b: Color32, t: f32) -> Color32 {
    style::mix(a, b, t)
}

fn show_node(ui: &mut Ui, node: &mut Node, rect: Rect, cx: &mut PaneCx) {
    match node {
        Node::Leaf(id) => show_pane(ui, *id, rect, cx),
        Node::Split { dir, ratio, a, b } => {
            let (ra, rsep, rb) = match dir {
                Dir::Horizontal => {
                    let w = rect.width() - GAP;
                    let wa = (w * *ratio).round();
                    (
                        Rect::from_min_size(rect.min, Vec2::new(wa, rect.height())),
                        Rect::from_min_size(Pos2::new(rect.min.x + wa, rect.min.y), Vec2::new(GAP, rect.height())),
                        Rect::from_min_max(Pos2::new(rect.min.x + wa + GAP, rect.min.y), rect.max),
                    )
                }
                Dir::Vertical => {
                    let h = rect.height() - GAP;
                    let ha = (h * *ratio).round();
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
                match dir {
                    Dir::Horizontal => *ratio += d.x / (rect.width() - GAP).max(1.0),
                    Dir::Vertical => *ratio += d.y / (rect.height() - GAP).max(1.0),
                }
                *ratio = ratio.clamp(0.1, 0.9);
            }
            let hv = style::anim(ui.ctx(), sep_id.with("h"), resp.hovered() || resp.dragged(), style::D_FAST);
            ui.painter().rect_filled(rsep, 0.0, ui.visuals().panel_fill);
            let line = match dir {
                Dir::Horizontal => Rect::from_center_size(rsep.center(), Vec2::new(1.0 + 2.0 * hv, rsep.height())),
                Dir::Vertical => Rect::from_center_size(rsep.center(), Vec2::new(rsep.width(), 1.0 + 2.0 * hv)),
            };
            let base = ui.visuals().widgets.noninteractive.bg_stroke.color;
            ui.painter().rect_filled(line, 0.0, style::mix(base, style::ACCENT, hv));
            show_node(ui, a, ra, cx);
            show_node(ui, b, rb, cx);
        }
    }
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
    let hresp = ui.interact(header, Id::new(("hdr", id)), if cx.detached { Sense::click() } else { Sense::click_and_drag() });
    if hresp.drag_started() {
        *cx.dragging = Some(id);
    }
    if hresp.clicked() {
        cx.actions.push(Action::Focus(id));
    }
    if tl_enabled {
        hresp.context_menu(|ui| {
            if ui.button(lang.t("Ticket verknüpfen …", "Link ticket …")).clicked() {
                cx.actions.push(Action::LinkTicket(id));
                ui.close();
            }
        });
    }
    let claude = entry.meta.claude && cx.claude_mode;
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
        style::paint_icon(ui.painter(), SIcon::SparkSmall, Pos2::new(hx + 4.0, cy), style::ACCENT);
        hx += 14.0;
    }
    let name_col = mix_col(text_c, strong_c, fo);
    // Breite des Knopfbereichs rechts: vier Knöpfe, mit Ticket-Verknüpfung ein fünfter.
    let tl_btn = tl_enabled;
    let btns_w = if cx.detached { 30.0 } else { 104.0 + if tl_btn { 26.0 } else { 0.0 } };
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
    if !entry.meta.queue.is_empty() {
        infos.push(format!("Queue {}", entry.meta.queue.len()));
    }
    if cx.claude_mode {
        if let Some(u) = cx.usage.get(id) {
            infos.push(usage_line(&u, lang));
        }
    }
    if let Some(g) = cx.git.get(id) {
        infos.push(git_line(&g));
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
                if cx.detached {
                    if style::icon_btn_small(ui, SIcon::Close, lang.t("Wieder andocken (Strg+Umschalt+W)", "Dock back (Ctrl+Shift+W)")).clicked() {
                        cx.actions.push(Action::Attach(id));
                    }
                    return;
                }
                if style::icon_btn_small(ui, SIcon::Close, lang.t("Sitzung schließen (Strg+Umschalt+W blendet nur den Bereich aus)", "Close session (Ctrl+Shift+W only hides the pane)")).clicked() {
                    cx.actions.push(Action::RequestClose(id));
                }
                if style::icon_btn_small(ui, SIcon::Detach, lang.t("In eigenem Fenster öffnen", "Open in own window")).clicked() {
                    cx.actions.push(Action::Detach(id));
                }
                if style::icon_btn_small(ui, SIcon::SplitDown, lang.t("Darunter teilen (Strg+Umschalt+E)", "Split below (Ctrl+Shift+E)")).clicked() {
                    cx.actions.push(Action::Split(id, Dir::Vertical));
                }
                if style::icon_btn_small(ui, SIcon::SplitRight, lang.t("Rechts teilen (Strg+Umschalt+D)", "Split right (Ctrl+Shift+D)")).clicked() {
                    cx.actions.push(Action::Split(id, Dir::Horizontal));
                }
                if tl_btn && style::icon_btn_small(ui, SIcon::Link, lang.t("Ticket verknüpfen …", "Link ticket …")).clicked() {
                    cx.actions.push(Action::LinkTicket(id));
                }
            });
        },
    );

    // Terminal
    let cwd_now = entry.cwd_now();
    let profile_id = entry.meta.profile;
    match entry.live.as_mut() {
        Some(session) => {
            let tid = Id::new(("term", id));
            if *cx.want_focus == Some(id) {
                ui.memory_mut(|m| m.request_focus(tid));
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
            let resp = termview::show(ui, body, tid, session, theme_local.as_ref().unwrap_or(cx.theme), font_local.as_ref().unwrap_or(cx.font), &cwd_now);
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
                        if ui.button(lang.t("Neu starten", "Restart")).clicked() {
                            cx.actions.push(Action::Restart(id));
                        }
                    });
                });
            }
        }
        None => {
            ui.painter().rect_filled(body, 0.0, cx.theme.bg);
            let msg = entry.error.clone().unwrap_or_else(|| "—".into());
            ui.scope_builder(UiBuilder::new().max_rect(body.shrink(12.0)), |ui| {
                ui.colored_label(Color32::LIGHT_RED, format!("{}: {msg}", lang.t("Start fehlgeschlagen", "Failed to start")));
                if ui.button(lang.t("Neu starten", "Restart")).clicked() {
                    cx.actions.push(Action::Restart(id));
                }
            });
        }
    }

    if !cx.detached {
        edge_buttons(ui, id, body, cx);
    }
    if cx.broadcast {
        ui.painter().rect_stroke(rect, 0.0, Stroke::new(1.5_f32, Color32::from_rgb(0xd9, 0x77, 0x57)), StrokeKind::Inside);
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
            if changed || step != 0 {
                st.matches = search::find_all(&session.term.lock().unwrap_or_else(|e| e.into_inner()), &st.query);
                st.last_query = st.query.clone();
                let n = st.matches.len();
                if n == 0 {
                    st.current = 0;
                } else if changed {
                    st.current = n - 1;
                } else {
                    st.current = (st.current as i32 + step).rem_euclid(n as i32) as usize;
                }
                if let Some(m) = st.matches.get(st.current) {
                    search::scroll_to(&mut session.term.lock().unwrap_or_else(|e| e.into_inner()), m.line);
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

fn tree_children(ui: &mut Ui, dir: &Path, hidden: bool, cache: &mut HashMap<PathBuf, Vec<TreeItem>>, actions: &mut Vec<Action>, lang: Lang) {
    let items = cache.entry(dir.to_path_buf()).or_insert_with(|| list_dir(dir, hidden)).clone();
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
            #[cfg(windows)]
            if ui.button(lang.t("Im Explorer anzeigen", "Show in Explorer")).clicked() {
                let _ = std::process::Command::new("explorer").arg(if it.is_dir { path_s.clone() } else { format!("/select,{path_s}") }).spawn();
                ui.close();
            }
        };
        if it.is_dir {
            let r = egui::CollapsingHeader::new(RichText::new(&it.name).strong()).id_salt(("tree", &it.path)).show(ui, |ui| {
                tree_children(ui, &it.path, hidden, cache, actions, lang);
            });
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

/// Startet die Shell mit erhöhten Rechten in einem externen Fenster (nur Windows).
fn launch_elevated(shell: &ShellSpec, cwd: &str) -> String {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let q = |s: &str| s.replace('\'', "''");
        let mut script = format!("Start-Process -FilePath '{}' -Verb RunAs", q(&shell.program));
        if !shell.args.is_empty() {
            script += &format!(" -ArgumentList '{}'", q(&shell.args.join(" ")));
        }
        if std::path::Path::new(cwd).is_dir() {
            script += &format!(" -WorkingDirectory '{}'", q(cwd));
        }
        let ok = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &script])
            .creation_flags(0x0800_0000)
            .spawn()
            .is_ok();
        return if ok {
            state::t("Als Administrator in einem eigenen Fenster gestartet (UAC-Abfrage). Dieses Terminal ist nicht eingebettet.", "Started as administrator in its own window (UAC prompt). This terminal is not embedded.").to_string()
        } else {
            state::t("Start als Administrator fehlgeschlagen.", "Starting as administrator failed.").to_string()
        };
    }
    #[cfg(not(windows))]
    {
        let _ = (shell, cwd);
        state::t("Start als Administrator wird nur unter Windows unterstützt.", "Starting as administrator is only supported on Windows.").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

/// „+“-Knopf am Rand eines Terminals: erscheint, wenn die Maus nahe an einem Rand ist, und teilt dort den Bereich.
fn edge_buttons(ui: &mut Ui, id: u64, body: Rect, cx: &mut PaneCx) {
    const ZONE: f32 = 30.0;
    let Some(p) = ui.input(|i| i.pointer.hover_pos()) else { return };
    if !body.contains(p) || cx.dragging.is_some() {
        return;
    }
    let lang = cx.lang;
    // nächster Rand: (Abstand, Seite)
    let cands = [(p.x - body.min.x, 0u8), (body.max.x - p.x, 1), (p.y - body.min.y, 2), (body.max.y - p.y, 3)];
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
    painter.circle_filled(center, r, style::ACCENT.gamma_multiply((0.55 + 0.4 * hover) * appear));
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

/// Button mit Zwei-Klick-Bestätigung (die Bestätigung verfällt nach 4 s).
/// Geometrie des Ordnerbaums der letzten Frame (für die Ablage-Linie beim Ziehen).
#[derive(Default)]
struct TreeGeo {
    sess: std::collections::HashMap<u64, Rect>,
    head: std::collections::HashMap<String, Rect>,
    full: std::collections::HashMap<String, Rect>,
    x: std::collections::HashMap<String, f32>,
    right: f32,
}

fn armed_button(ui: &mut Ui, key: impl std::hash::Hash, label: &str, armed_label: &str) -> bool {
    let id = egui::Id::new(("armed", key));
    let now = ui.input(|i| i.time);
    let armed = ui.memory(|m| m.data.get_temp::<f64>(id)).is_some_and(|t| now - t < 4.0);
    if ui.button(if armed { armed_label } else { label }).clicked() {
        if armed {
            ui.memory_mut(|m| m.data.remove::<f64>(id));
            return true;
        }
        ui.memory_mut(|m| m.data.insert_temp(id, now));
    }
    false
}
