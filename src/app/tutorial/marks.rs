//! Ziele der Hervorhebung (#143): Bedienelemente melden pro Bild ihr Rechteck unter einer festen Kennung (`Spot`).
//!
//! Gemeldet wird nur, solange eine Tour läuft, und nur im Hauptfenster (Viewport `ROOT`) – weitere Fenster, abgedockte
//! Sitzungen und Tests ohne Tour zeichnen unverändert. Die Liste lebt je Thread (die Oberfläche läuft in einem Thread,
//! Tests laufen parallel in eigenen Threads) und wird zu Beginn jedes Bilds geleert (`begin_frame`), die Tour liest sie
//! am Ende des Bilds (`seen`).
//!
//! Aufrufer: `style::icon_btn(ui, …).spot(Spot::SidebarNew)` für eine `Response`, `spot_rect(ui, Spot::X, rect)` für selbst
//! gezeichnete Flächen. Der Test `every_spot_is_marked_in_the_ui` prüft im Quelltext, dass jede Kennung irgendwo außerhalb
//! dieses Moduls gemeldet wird.

use eframe::egui::{self, Align, Rect, Response, Ui, ViewportId};
use std::cell::RefCell;
use std::collections::HashMap;

macro_rules! spots {
    ($($v:ident = $id:literal),* $(,)?) => {
        /// Kennung eines hervorhebbaren Bedienelements.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub(in crate::app) enum Spot { $($v,)* }

        impl Spot {
            /// Alle Kennungen (für die Tests).
            #[cfg_attr(not(test), allow(dead_code))]
            pub(in crate::app) const ALL: &'static [Spot] = &[$(Spot::$v,)*];

            /// Stabile Kennung als Text (Statusdatei des Test-Builds, Fehlermeldungen der Tests).
            #[cfg_attr(not(any(test, feature = "tutorial-test")), allow(dead_code))]
            pub(in crate::app) fn id(self) -> &'static str {
                match self { $(Spot::$v => $id,)* }
            }

            /// Name der Variante (so steht sie im Quelltext: `Spot::<name>`).
            #[cfg_attr(not(test), allow(dead_code))]
            pub(in crate::app) fn name(self) -> &'static str {
                match self { $(Spot::$v => stringify!($v),)* }
            }
        }
    };
}

spots! {
    Sidebar = "sidebar",
    SidebarNew = "sidebar.new",
    SidebarClaude = "sidebar.claude",
    SidebarChat = "sidebar.chat",
    SidebarComposer = "sidebar.composer",
    SidebarPalette = "sidebar.palette",
    SidebarPrompts = "sidebar.prompts",
    SidebarFiles = "sidebar.files",
    SidebarDiff = "sidebar.diff",
    SidebarHistory = "sidebar.history",
    SidebarFooter = "sidebar.footer",
    ProcsSection = "sidebar.processes",
    TemplatesSection = "sidebar.templates",
    ProjectBar = "sidebar.projects",
    SessionRow = "sidebar.session",
    PaneSplit = "pane.split_right",
    PaneMinimize = "pane.minimize",
    TitlebarTabs = "titlebar.tabs",
    TitlebarTodo = "titlebar.todo",
    Palette = "palette",
    TabShortcuts = "settings.tab.shortcuts",
    TabProfiles = "settings.tab.profiles",
    TabModels = "settings.tab.models",
    ClaudeMode = "settings.claude.mode",
    ClaudeDefaults = "settings.claude.defaults",
    ClaudeBudget = "settings.claude.budget",
    McpLevel = "settings.agents.level",
    McpRegister = "settings.agents.register",
    McpSysprompt = "settings.agents.system_prompt",
    // Umgebungsvariablen der Sitzungen (#384, 3.2.0)
    McpSessionEnv = "settings.agents.session_env",
    McpLog = "settings.agents.log",
    ModelsServers = "settings.models.servers",
    ModelsCommands = "settings.models.commands",
    DataBackups = "settings.data.backups",
    DataCleanup = "settings.data.cleanup",
    AboutInfo = "settings.about.info",
    AboutUpdates = "settings.about.updates",
    AboutTutorial = "settings.about.tutorial",
    ThemeManage = "settings.appearance.theme",
    ReduceTransparency = "settings.appearance.reduce_transparency",
    TodoInput = "todo.input",
    TodoList = "todo.list",
    ExpertToggle = "todo.expert",
    PlanProjects = "plan.projects",
    PlanNewTicket = "plan.new_ticket",
    PlanTabBoards = "plan.tab.boards",
    PlanTabBacklog = "plan.tab.backlog",
    PlanTabSearch = "plan.tab.search",
    QueryBar = "plan.query",
    // Editor-Reiter (#54, 3.0.0)
    TitlebarEditor = "titlebar.editor",
    EditorTree = "editor.tree",
    EditorTabBar = "editor.tabs",
    EditorSplit = "editor.split_right",
    EditorText = "editor.text",
    EditorFind = "editor.find",
    EditorStatus = "editor.status",
    EditorMinimap = "editor.minimap",
    EditorSearchFiles = "editor.search_files",
    TabEditor = "settings.tab.editor",
    EditorSearchSettings = "settings.editor.search",
    EditorAgentSwitches = "settings.editor.agents",
    // Reiter „Zeit“ (#164, 3.2.0)
    TitlebarTime = "titlebar.time",
    TimeSubTabs = "time.subtabs",
    TimeNewAlarm = "time.new_alarm",
    TimePresets = "time.presets",
    CalendarViews = "time.calendar.views",
    TimeOptions = "time.options",
    // Menüleiste je Teilung (#381, 3.2.0)
    EditorMenuBar = "editor.menu_bar",
}

/// Wo ein Ziel in diesem Bild lag: ganzes Rechteck und der sichtbare (nicht weggeschnittene) Teil.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::app) struct Seen {
    #[cfg_attr(not(any(test, feature = "tutorial-test")), allow(dead_code))]
    pub rect: Rect,
    pub shown: Rect,
}

#[derive(Default)]
struct Reg {
    /// Läuft eine Tour (sonst wird nichts gemeldet)?
    on: bool,
    /// Ziel des aktuellen Schritts: wird einmal in den sichtbaren Bereich gerollt.
    want: Option<Spot>,
    scrolled: bool,
    cur: HashMap<Spot, Seen>,
}

thread_local! {
    static REG: RefCell<Reg> = RefCell::new(Reg::default());
}

/// Zu Beginn eines Bilds: alte Meldungen verwerfen; `on` = Tour läuft, `want` = Ziel des aktuellen Schritts.
pub(in crate::app) fn begin_frame(on: bool, want: Option<Spot>) {
    REG.with(|r| {
        let mut r = r.borrow_mut();
        if r.want != want {
            r.scrolled = false;
        }
        r.on = on;
        r.want = if on { want } else { None };
        r.cur.clear();
    });
}

/// Wo `s` in diesem Bild gemeldet wurde (zuletzt gemeldete Stelle gewinnt).
pub(in crate::app) fn seen(s: Spot) -> Option<Seen> {
    REG.with(|r| r.borrow().cur.get(&s).copied())
}

/// Meldung eintragen; `true`, wenn das Ziel jetzt einmal in den sichtbaren Bereich gerollt werden soll.
fn record(ctx: &egui::Context, s: Spot, rect: Rect, clip: Rect) -> bool {
    if ctx.viewport_id() != ViewportId::ROOT || !rect.is_positive() {
        return false;
    }
    REG.with(|r| {
        let mut r = r.borrow_mut();
        if !r.on {
            return false;
        }
        let shown = rect.intersect(clip);
        r.cur.insert(s, Seen { rect, shown });
        let hidden = !shown.is_positive() || shown.area() < rect.area() * 0.6;
        if r.want == Some(s) && !r.scrolled && hidden {
            r.scrolled = true;
            return true;
        }
        false
    })
}

/// Selbst gezeichnete Fläche `rect` als Ziel `s` melden (sichtbar ist der Teil im Clip-Bereich von `ui`).
pub(in crate::app) fn spot_rect(ui: &Ui, s: Spot, rect: Rect) {
    if record(ui.ctx(), s, rect, ui.clip_rect()) {
        ui.scroll_to_rect(rect, Some(Align::Center));
    }
}

/// Was `ui` seit der Höhe `top` gezeichnet hat (volle Breite von `ui`) als Ziel `s` melden – für Abschnitte:
/// `let top = ui.cursor().min.y; …; spot_since(ui, Spot::X, top)`.
pub(in crate::app) fn spot_since(ui: &Ui, s: Spot, top: f32) {
    let r = ui.min_rect();
    spot_rect(ui, s, Rect::from_min_max(egui::pos2(r.left(), top), egui::pos2(r.right(), ui.cursor().min.y.max(top + 1.0))));
}

/// Bedienelement als Ziel melden: `ui.button(…).spot(Spot::X).clicked()`.
pub(in crate::app) trait TutMark {
    fn spot(self, s: Spot) -> Self;
}

impl TutMark for Response {
    fn spot(self, s: Spot) -> Self {
        if record(&self.ctx, s, self.rect, self.interact_rect) {
            self.scroll_to_me(Some(Align::Center));
        }
        self
    }
}
