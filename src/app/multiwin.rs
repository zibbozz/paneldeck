//! Weitere Fenster (#110): Strg+Klick auf einen Reiter der Fensterleiste öffnet ihn in einem neuen Fenster.
//!
//! Alle Fenster laufen im selben Prozess als zusätzliche egui-Viewports (`show_viewport_immediate`) und teilen sich einen
//! Zustand im Speicher: Einstellungen, Datenbank, Tray, MCP, IPC und alle Sitzungen (`App::entries`) gibt es genau einmal.
//! Eigen ist jedem Fenster nur seine Ansicht (`WinView`): Layout, Fokus, aktives Projekt, Reiter, Seitenleisten-Filter,
//! offene Dialoge sowie die Zustände des Todo-Reiters. Jede Sitzung gehört genau einem Fenster (`SessionMeta::window`,
//! 0 = Hauptfenster); Seitenleiste, Layout und Kürzel eines Fensters sehen nur seine Sitzungen.
//!
//! Umsetzung: Die Felder der App beschreiben immer das *gerade gezeichnete* Fenster. Vor dem Zeichnen eines weiteren Fensters
//! tauscht `enter` dessen `WinView` mit den Feldern der App (das Hauptfenster wandert in die `WinView`), `leave` tauscht zurück.
//! So läuft der vorhandene Code (Seitenleiste, Bereiche, Dialoge, `apply`) unverändert für jedes Fenster. Alles Globale
//! (Hooks, MCP, Tray, Speichern) läuft nur mit dem Hauptfenster in den Feldern (`wins.cur == 0`).
//!
//! Aktionen, die eine Sitzung eines anderen Fensters zeigen sollen (aktivieren, anheften, Sprung aus Suche/Ticket), werden an
//! dieses Fenster weitergereicht (`route`) und dort im nächsten Bild ausgeführt; das Fenster kommt dabei nach vorn.

use super::*;
use crate::state::{SavedProjectView, SavedWindow};
use std::sync::{Arc, OnceLock};

/// Felder einer Fensteransicht, die 1:1 mit den gleichnamigen Feldern von `App` getauscht werden (`swap_view`).
/// `proj_views` fehlt hier: es wird mit `Project::layout`/`focused` getauscht (`swap_project_views`).
/// Die Liste steht nur hier; `swap_fields!` zerlegt `WinView` ohne `..`, ein vergessenes Feld ist daher ein Übersetzungsfehler.
macro_rules! view_fields {
    ($mac:ident!($($args:tt)*)) => {
        $mac!($($args)*;
            layout, focused, want_focus, active_project, mode, filter, pane_rects, last_title, rename, group_prompt, new_dialog, new_chat,
            confirm_close, settings_open, settings_tab, capture_key, profile_sel, palette, text_prompt, folder_prompt, notes_open,
            queue_open, queue_draft, files_open, diff_open, diff_sel, composer_open, composer_focus, composer_text, history_open,
            history_filter, history_hits, history_hits_rx, templates_open, tpl_name, cost_open, gsearch, prompts_open, prompt_sel,
            broadcast, wt_close, editor, confirm_restore, restore_error, dragging, dragging_folder, folder_rects, tree_geo,
            tree_root, plan, tasks
        )
    };
}

/// `$a` (App oder – in Tests – eine zweite `WinView`) und `$v` feldweise tauschen; `$pv` bindet `proj_views` für `$pv_body`.
macro_rules! swap_fields {
    ($a:expr, $v:expr, |$pv:ident| $pv_body:expr; $($f:ident),* $(,)?) => {{
        let WinView { proj_views: $pv, $($f),* } = $v;
        $( std::mem::swap(&mut $a.$f, $f); )*
        $pv_body;
    }};
}

/// Ansicht eines Fensters. Die Feldnamen entsprechen den Feldern von `App` (siehe `view_fields!`).
pub(super) struct WinView {
    layout: Option<Node>,
    focused: u64,
    want_focus: Option<u64>,
    active_project: u64,
    mode: titlebar::Mode,
    filter: String,
    pane_rects: HashMap<u64, Rect>,
    last_title: String,
    rename: Option<(u64, String)>,
    group_prompt: Option<Prompt>,
    new_dialog: Option<NewDialog>,
    new_chat: Option<models_ui::NewChatDlg>,
    confirm_close: Option<u64>,
    settings_open: bool,
    settings_tab: usize,
    /// Laufende Kürzel-Aufnahme in den Einstellungen dieses Fensters (sperrt nur hier die Kürzel).
    capture_key: Option<String>,
    profile_sel: usize,
    palette: Option<Palette>,
    text_prompt: Option<TextPrompt>,
    folder_prompt: Option<FolderPrompt>,
    notes_open: Option<u64>,
    queue_open: Option<u64>,
    queue_draft: String,
    files_open: bool,
    diff_open: bool,
    diff_sel: Option<(String, bool)>,
    composer_open: bool,
    composer_focus: bool,
    composer_text: String,
    history_open: bool,
    history_filter: String,
    history_hits: Option<Vec<(history::HistoryItem, String)>>,
    history_hits_rx: Option<std::sync::mpsc::Receiver<Vec<(history::HistoryItem, String)>>>,
    templates_open: bool,
    tpl_name: String,
    cost_open: bool,
    gsearch: Option<GlobalSearch>,
    prompts_open: bool,
    prompt_sel: usize,
    broadcast: bool,
    wt_close: Option<WtClose>,
    editor: Option<Editor>,
    /// Rückfrage „Sicherung wiederherstellen“ erscheint im Fenster, in dem sie angeklickt wurde.
    confirm_restore: Option<PathBuf>,
    restore_error: Option<String>,
    dragging: Option<u64>,
    dragging_folder: Option<String>,
    folder_rects: Vec<(String, Rect)>,
    tree_geo: TreeGeo,
    tree_root: String,
    plan: planning_ui::PlanUi,
    tasks: tasks_ui::TasksUi,
    /// Layouts der Projekte, die in diesem Fenster gerade nicht aktiv sind: (Projekt, Layout, Fokus).
    /// Die App hält diese in `Project::layout`/`focused`; `swap_project_views` tauscht sie mit.
    proj_views: Vec<(u64, Option<Node>, u64)>,
}

impl WinView {
    /// Leere Ansicht: keine Sitzung (#111), gewünschter Reiter, Projekt wie im Fenster, aus dem sie geöffnet wurde.
    pub(super) fn new(mode: titlebar::Mode, project: u64) -> Self {
        WinView {
            layout: None,
            focused: 0,
            want_focus: None,
            active_project: project,
            mode,
            filter: String::new(),
            pane_rects: HashMap::new(),
            last_title: String::new(),
            rename: None,
            group_prompt: None,
            new_dialog: None,
            new_chat: None,
            confirm_close: None,
            settings_open: false,
            settings_tab: 0,
            capture_key: None,
            profile_sel: 0,
            palette: None,
            text_prompt: None,
            folder_prompt: None,
            notes_open: None,
            queue_open: None,
            queue_draft: String::new(),
            files_open: false,
            diff_open: false,
            diff_sel: None,
            composer_open: false,
            composer_focus: false,
            composer_text: String::new(),
            history_open: false,
            history_filter: String::new(),
            history_hits: None,
            history_hits_rx: None,
            templates_open: false,
            tpl_name: String::new(),
            cost_open: false,
            gsearch: None,
            prompts_open: false,
            prompt_sel: 0,
            broadcast: false,
            wt_close: None,
            editor: None,
            confirm_restore: None,
            restore_error: None,
            dragging: None,
            dragging_folder: None,
            folder_rects: Vec::new(),
            tree_geo: TreeGeo::default(),
            tree_root: String::new(),
            plan: planning_ui::PlanUi::default(),
            tasks: tasks_ui::TasksUi::default(),
            proj_views: Vec::new(),
        }
    }
}

/// Ansicht `v` mit den Feldern der App tauschen (zweimal aufgerufen = wieder wie vorher).
fn swap_view(app: &mut App, v: &mut WinView) {
    view_fields!(swap_fields!(app, v, |pv| swap_project_views(&mut app.projects, pv)));
}

/// Layout/Fokus je Projekt tauschen: danach stehen in `projects` die Werte aus `stash` (fehlende: kein Layout) und in `stash`
/// die bisherigen der Projekte. Gelöschte Projekte fallen weg, neue bekommen im anderen Fenster kein Layout.
pub(super) fn swap_project_views(projects: &mut [Project], stash: &mut Vec<(u64, Option<Node>, u64)>) {
    let mut out = Vec::with_capacity(projects.len());
    for p in projects.iter_mut() {
        let (l, f) = stash.iter_mut().find(|s| s.0 == p.id).map(|s| (s.1.take(), s.2)).unwrap_or((None, 0));
        out.push((p.id, std::mem::replace(&mut p.layout, l), std::mem::replace(&mut p.focused, f)));
    }
    *stash = out;
}

/// Blätter entfernen, die nicht (mehr) in die Ansicht gehören, dazu doppelte; Anteile begrenzen (`Node::normalize`).
/// `None`, wenn nichts übrig bleibt. Ist schon alles in Ordnung, bleibt der Baum unangetastet.
pub(super) fn prune_layout(layout: Node, ok: impl Fn(u64) -> bool) -> Option<Node> {
    if layout.is_normal(&ok) { Some(layout) } else { layout.normalize(ok) }
}

/// Paneldeck-Fenster im Vordergrund (0 = Hauptfenster), `None` = keins. Einmal je Bild zu Beginn des Hauptbildes bestimmt
/// (Hauptfenster: Stand dieses Bildes, weitere Fenster: Stand ihres letzten Bildes) und dann überall benutzt – so zeichnet
/// genau ein Fenster Hinweise und Toasts, auch im Bild, in dem der Fokus wechselt.
pub(super) fn foreground(main_focused: bool, others: impl Iterator<Item = (u64, bool)>) -> Option<u64> {
    if main_focused {
        return Some(0);
    }
    others.filter(|(_, f)| *f).map(|(id, _)| id).next()
}

/// Weitergereichtes für geschlossene Fenster verwerfen. Liefert, wie viele Einträge wegfielen.
pub(super) fn drop_closed<T>(queue: &mut Vec<(u64, T)>, alive: impl Fn(u64) -> bool) -> usize {
    let before = queue.len();
    queue.retain(|(w, _)| *w == 0 || alive(*w));
    before - queue.len()
}

/// Fenster, an das eine Aktion weitergereicht werden muss (`None` = hier ausführen).
/// `session_window`: Fenster der Zielsitzung (falls es sie gibt); abgedockte Sitzungen holt jedes Fenster selbst nach vorn.
pub(super) fn route_target(action: &Action, session_window: impl Fn(u64) -> Option<u64>, detached: impl Fn(u64) -> bool, cur: u64) -> Option<u64> {
    let (id, follows_detached) = match action {
        Action::Activate(id) | Action::Focus(id) | Action::JumpTo(id) => (*id, true),
        Action::GlobalJump(id, ..) => (*id, true),
        Action::Attach(id) => (*id, false),
        _ => return None,
    };
    if follows_detached && detached(id) {
        return None;
    }
    session_window(id).filter(|w| *w != cur)
}

/// Strg+Klick auf einen Reiter öffnet ihn in einem neuen Fenster, ein normaler Klick wechselt nur den Reiter.
pub(super) fn opens_window(modifiers: Modifiers) -> bool {
    modifiers.command && !modifiers.shift && !modifiers.alt
}

pub(super) fn mode_key(m: titlebar::Mode) -> &'static str {
    match m {
        titlebar::Mode::Terminals => "terminals",
        titlebar::Mode::Tasks => "tasks",
    }
}

pub(super) fn mode_from_key(s: &str) -> titlebar::Mode {
    if s == "tasks" { titlebar::Mode::Tasks } else { titlebar::Mode::Terminals }
}

/// Fenster und Projekt einer Sitzung, die ein Agent anlegt: die seiner eigenen Hub-Sitzung (`caller` = deren Fenster und
/// Projekt), sonst das Hauptfenster mit dessen aktivem Projekt.
pub(super) fn child_home(caller: Option<(u64, u64)>, main_project: u64) -> (u64, u64) {
    caller.unwrap_or((0, main_project))
}

/// Fenster einer neuen Sitzung: das gewünschte, wenn es (noch) existiert, sonst das gerade gezeichnete.
pub(super) fn pick_window(want: Option<u64>, cur: u64, exists: impl Fn(u64) -> bool) -> u64 {
    match want {
        Some(w) if w == cur || w == 0 || exists(w) => w,
        _ => cur,
    }
}

/// Was beim Schließen eines weiteren Fensters mit seinen Sitzungen geschieht (#110).
#[derive(Default, Debug, PartialEq)]
pub(super) struct ClosePlan {
    /// Werden beendet.
    pub(super) end: Vec<u64>,
    /// Davon laufen noch.
    pub(super) running: usize,
    /// Abgedockte Sitzungen: bleiben in ihren eigenen Fenstern offen und gehören danach dem Hauptfenster.
    pub(super) adopt: Vec<u64>,
    /// Sitzungen mit Git-Worktree: erst über die Worktree-Rückfrage schließen (behalten, entfernen, mergen) – nie still.
    pub(super) worktrees: Vec<u64>,
    /// Davon Chats mit Verlauf (der beim Schließen gelöscht wird).
    pub(super) chats: usize,
}

impl ClosePlan {
    /// Rückfrage nötig? Immer, wenn noch etwas läuft, abgedockt ist oder einen Worktree hat (unabhängig von
    /// „Schließen bestätigen“: ein ganzes Fenster beendet viele Sitzungen auf einmal).
    pub(super) fn ask(&self) -> bool {
        self.running > 0 || self.chats > 0 || !self.adopt.is_empty() || !self.worktrees.is_empty()
    }

    /// Darf das Fenster jetzt schließen? Nicht, solange Worktree-Sitzungen offen sind.
    pub(super) fn can_close(&self) -> bool {
        self.worktrees.is_empty()
    }
}

/// Plan aus den Sitzungen eines Fensters: (Id, läuft, hat Worktree, abgedockt).
pub(super) fn close_plan(sessions: impl Iterator<Item = (u64, bool, bool, bool)>) -> ClosePlan {
    let mut p = ClosePlan::default();
    for (id, running, worktree, detached) in sessions {
        if worktree {
            p.worktrees.push(id);
        } else if detached {
            p.adopt.push(id);
        } else {
            p.end.push(id);
            p.running += running as usize;
        }
    }
    p
}

/// Mindestgröße eines Fensters (Punkte).
const MIN_SIZE: Vec2 = Vec2::new(640.0, 400.0);

/// Gespeicherte Lage eines Fensters an die Bildschirme anpassen. `pos`/`size` in egui-Punkten, `ppp` = physische Pixel je
/// Punkt (Bildschirmskalierung × Zoom), `on_screen`/`work` fragen in physischen Pixeln (`winctl`). Liegt die Position auf
/// keinem Monitor mehr, wählt das System sie; die Größe passt in die Arbeitsfläche des Monitors, und das Fenster wird so
/// verschoben, dass es ganz darauf liegt.
pub(super) fn fit_window(pos: Option<Pos2>, size: Vec2, ppp: f32, on_screen: impl Fn(f32, f32) -> bool, work: impl Fn(f32, f32) -> Option<[f32; 4]>) -> (Option<Pos2>, Vec2) {
    let ppp = if ppp.is_finite() && ppp > 0.0 { ppp } else { 1.0 };
    let pos = pos.filter(|p| p.x.is_finite() && p.y.is_finite() && on_screen(p.x * ppp, p.y * ppp));
    let mut size = if size.x.is_finite() && size.y.is_finite() { size } else { Vec2::new(1100.0, 720.0) };
    let probe = pos.unwrap_or(Pos2::ZERO);
    let Some([l, t, r, b]) = work(probe.x * ppp, probe.y * ppp) else { return (pos, size.max(MIN_SIZE)) };
    let area = Rect::from_min_max(Pos2::new(l, t) / ppp, Pos2::new(r, b) / ppp);
    size = size.min(area.size()).max(MIN_SIZE);
    let pos = pos.map(|p| Pos2::new(p.x.min(area.max.x - size.x).max(area.min.x), p.y.min(area.max.y - size.y).max(area.min.y)));
    (pos, size)
}

/// Welche gespeicherten Fenster wiederhergestellt werden: `(Fenster, Projekt)` – ohne Id 0 (Hauptfenster) und doppelte Ids;
/// ein gelöschtes Projekt wird durch `first_project` ersetzt.
pub(super) fn restorable<'a>(saved: &'a [SavedWindow], project_ok: impl Fn(u64) -> bool, first_project: u64) -> Vec<(&'a SavedWindow, u64)> {
    let mut ids: Vec<u64> = Vec::new();
    let mut out = Vec::new();
    for s in saved {
        if s.id == 0 || ids.contains(&s.id) {
            continue;
        }
        ids.push(s.id);
        out.push((s, if project_ok(s.project) { s.project } else { first_project }));
    }
    out
}

/// Sitzungen, deren Fenster es nicht (mehr) gibt, gehören dem Hauptfenster.
pub(super) fn adopt_orphans<'a>(windows: impl Iterator<Item = &'a mut u64>, existing: &[u64]) {
    for w in windows {
        if *w != 0 && !existing.contains(w) {
            *w = 0;
        }
    }
}

/// Wunsch „neues Fenster“ (aus der Fensterleiste).
struct OpenReq {
    mode: titlebar::Mode,
    project: u64,
    /// Äußeres Rechteck des Fensters, aus dem geöffnet wurde (neues Fenster leicht versetzt daneben).
    from: Option<Rect>,
    size: Option<Vec2>,
}

/// Ein weiteres Fenster.
pub(super) struct HubWindow {
    pub(super) id: u64,
    view: WinView,
    /// Stand der Daten (`Wins::data_gen`), den die Todo-Ansicht dieses Fensters kennt.
    data_gen: u64,
    os_focused: bool,
    hwnd: Option<isize>,
    hwnd_tries: u32,
    init_pos: Option<Pos2>,
    init_size: Vec2,
    init_max: bool,
    /// Beim Start wiederhergestellt: ohne Fokus anlegen (bleibt für das Fenster gleich – egui baut es sonst neu).
    restored: bool,
    /// Verborgen, bis das Hauptfenster aus dem Tray geholt wird (Start mit `--tray`).
    hidden: bool,
    outer: Option<Rect>,
    inner: Option<Rect>,
    maximized: bool,
    /// Rückfrage „Fenster schließen?“ ist offen.
    ask_close: bool,
    close: bool,
    /// Layout beim ersten Zeichnen prüfen (wiederhergestellte Fenster).
    needs_load: bool,
    /// egui-Gedächtnis dieses Fensters (`Memory::data`: Bildlauf, Textfelder, aufgeklappte Abschnitte, Bereichsgrößen …).
    /// egui hält es für alle Fenster gemeinsam und nach Id; gleiche Ids in zwei Fenstern teilten sich sonst den Zustand.
    /// Wird für die Dauer des Fensterbildes mit dem des Hauptfensters getauscht.
    mem: egui::util::IdTypeMap,
}

impl HubWindow {
    pub(super) fn new(id: u64, view: WinView, pos: Option<Pos2>, size: Vec2, maximized: bool) -> Self {
        HubWindow { id, view, data_gen: 0, os_focused: false, hwnd: None, hwnd_tries: 0, init_pos: pos, init_size: size, init_max: maximized, restored: false, hidden: false, outer: None, inner: None, maximized, ask_close: false, close: false, needs_load: true, mem: Default::default() }
    }
    fn viewport(&self) -> egui::ViewportId {
        viewport_of(self.id)
    }
}

pub(super) fn viewport_of(window: u64) -> egui::ViewportId {
    if window == 0 { egui::ViewportId::ROOT } else { egui::ViewportId::from_hash_of(("hubwin", window)) }
}

/// Alle weiteren Fenster und was zwischen ihnen vermittelt wird.
#[derive(Default)]
pub(super) struct Wins {
    /// Gerade gezeichnetes Fenster (0 = Hauptfenster).
    pub(super) cur: u64,
    list: Vec<HubWindow>,
    next: u64,
    /// Weitergereichte Aktionen: (Zielfenster, Aktion).
    queue: Vec<(u64, Action)>,
    open_req: Option<OpenReq>,
    /// Zähler für Datenänderungen (Schreiben in die Datenbank aus einem Fenster, Änderungen durch Agenten); Fenster mit
    /// älterem Stand laden die Todo-Ansicht neu, damit Änderungen aus einem Fenster im anderen erscheinen.
    pub(super) data_gen: u64,
    /// Zuletzt gesehener Stand von `total_changes` der Datenbank.
    db_changes: u64,
    main_gen: u64,
    /// Fenster im Vordergrund (siehe `foreground`); bestimmt zu Beginn jedes Hauptbildes.
    fg: Option<u64>,
    /// Fenster, das zuletzt Hinweise und Toasts gezeichnet hat.
    overlay_last: u64,
    /// „Rückgängig“ eines Toasts, dessen Änderung in einem anderen Fenster entstand: (Fenster, Token).
    undo: Vec<(u64, u64)>,
    /// „Sicherung wiederherstellen“ aus einem weiteren Fenster: nach dessen Bild speichern und beenden.
    pub(super) exit_after_save: bool,
}

impl Wins {
    pub(super) fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
}

fn icon() -> Arc<egui::IconData> {
    static ICON: OnceLock<Arc<egui::IconData>> = OnceLock::new();
    ICON.get_or_init(|| Arc::new(eframe::icon_data::from_png_bytes(include_bytes!("../../assets/icon64.png")).unwrap_or_default())).clone()
}

/// Hat sich der Datenbestand seit dem letzten Stand geändert? `total` = Zahl aller geänderten Zeilen der Verbindung
/// (`sqlite3_total_changes`): sie wächst mit jedem Schreiben, ob aus einem Fenster oder durch einen Agenten.
pub(super) fn data_changed(last: &mut u64, total: u64) -> bool {
    let changed = *last != total;
    *last = total;
    changed
}

impl App {
    /// Gehört die Sitzung zum gerade gezeichneten Fenster?
    pub(super) fn in_win(&self, e: &Entry) -> bool {
        e.meta.window == self.wins.cur
    }

    /// Fenster der Sitzung (falls es sie gibt).
    pub(super) fn window_of(&self, id: u64) -> Option<u64> {
        self.entries.get(&id).map(|e| e.meta.window)
    }

    /// Aktion an das Fenster der Zielsitzung weiterreichen, wenn es ein anderes ist. `true` = weitergereicht.
    pub(super) fn route(&mut self, ctx: &egui::Context, a: &Action) -> bool {
        let Some(w) = route_target(a, |id| self.window_of(id), |id| self.detached.contains(&id), self.wins.cur) else { return false };
        self.route_to(ctx, w, a.clone());
        true
    }

    /// Aktion im nächsten Bild von Fenster `window` ausführen (das Fenster kommt dabei nach vorn).
    pub(super) fn route_to(&mut self, ctx: &egui::Context, window: u64, a: Action) {
        self.wins.queue.push((window, a));
        ctx.request_repaint();
    }

    /// Fenster und Projekt einer neuen Sitzung (siehe `NewOpts::window`/`project`): ein geschlossenes Fenster fällt auf das
    /// gerade gezeichnete zurück, ein fehlendes oder gelöschtes Projekt auf das aktive Projekt des Zielfensters.
    pub(super) fn session_home(&self, window: Option<u64>, project: Option<u64>) -> (u64, u64) {
        let other = |w: u64| self.wins.list.iter().find(|x| x.id == w);
        let w = pick_window(window, self.wins.cur, |w| other(w).is_some());
        let fallback = if w == self.wins.cur { self.active_project } else { other(w).map(|x| x.view.active_project).unwrap_or(self.active_project) };
        let p = project.filter(|p| self.projects.iter().any(|x| x.id == *p)).unwrap_or(fallback);
        (w, p)
    }

    /// Weitergereichte Aktionen für das gerade gezeichnete Fenster; das Fenster kommt dabei nach vorn.
    fn take_routed(&mut self, ctx: &egui::Context) -> Vec<Action> {
        let cur = self.wins.cur;
        let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.wins.queue).into_iter().partition(|(w, _)| *w == cur);
        self.wins.queue = rest;
        if mine.is_empty() {
            return Vec::new();
        }
        if cur == 0 && self.in_tray {
            self.from_tray(ctx);
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        mine.into_iter().map(|(_, a)| a).collect()
    }

    /// Strg+Klick auf einen Reiter: neues Fenster mit diesem Reiter vormerken (wird nach dem Bild angelegt).
    pub(super) fn request_window(&mut self, ctx: &egui::Context, mode: titlebar::Mode) {
        let (from, size) = ctx.input(|i| (i.viewport().outer_rect, i.viewport().inner_rect.map(|r| r.size())));
        self.wins.open_req = Some(OpenReq { mode, project: self.active_project, from, size });
        ctx.request_repaint();
    }

    /// Ganz zu Beginn des Hauptbildes: welches Fenster ist vorn (für „gesehen“, Benachrichtigungen, Hinweise und Toasts)?
    pub(super) fn track_focus(&mut self, ctx: &egui::Context) {
        let main = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        self.wins.fg = foreground(main, self.wins.list.iter().map(|w| (w.id, w.os_focused)));
        let overlay = self.overlay_window();
        if overlay != self.wins.overlay_last {
            self.wins.overlay_last = overlay;
            self.toasts_hover = None; // der Mauszeiger des anderen Fensters hält keinen Toast mehr an
        }
    }

    /// Fenster, das in diesem Bild Hinweise und Toasts zeichnet: das im Vordergrund, sonst das Hauptfenster.
    pub(super) fn overlay_window(&self) -> u64 {
        self.wins.fg.unwrap_or(0)
    }

    /// „Rückgängig“-Klicks, die diesem Fenster gehören, ausführen (der Rückgängig-Stapel gehört zur Todo-Ansicht des Fensters).
    fn run_routed_undo(&mut self, ctx: &egui::Context) {
        let cur = self.wins.cur;
        let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.wins.undo).into_iter().partition(|(w, _)| *w == cur);
        self.wins.undo = rest;
        for (_, token) in mine {
            self.undo_here(ctx, token);
        }
    }

    /// „Rückgängig“ für die Todo-Ansicht eines anderen Fensters vormerken (läuft in dessen nächstem Bild, ohne es nach vorn zu holen).
    pub(super) fn wins_undo_later(&mut self, ctx: &egui::Context, window: u64, token: u64) {
        self.wins.undo.push((window, token));
        ctx.request_repaint();
    }

    /// Hauptfenster, Beginn des Bildes: Ansicht prüfen, Todo-Ansicht ggf. neu laden, weitergereichte Aktionen.
    pub(super) fn main_frame_begin(&mut self, ctx: &egui::Context) -> Vec<Action> {
        debug_assert_eq!(self.wins.cur, 0);
        self.run_routed_undo(ctx);
        if self.wins.main_gen != self.wins.data_gen {
            self.wins.main_gen = self.wins.data_gen;
            self.plan.invalidate();
            self.tasks.invalidate();
        }
        if !self.wins.is_empty() {
            self.prune_view();
        }
        self.take_routed(ctx)
    }

    /// Hinweise und Toasts erscheinen im Fenster, das der Nutzer gerade benutzt: im Hauptfenster, außer ein weiteres Fenster
    /// ist im Vordergrund (dann zeichnet dieses sie, siehe `child_frame`).
    pub(super) fn main_overlay(&self) -> bool {
        self.overlay_window() == 0
    }

    /// Fenster, das für eine Sitzung in der Taskleiste blinken soll: ihr abgedocktes Fenster, sonst das Paneldeck-Fenster,
    /// zu dem sie gehört (nicht immer das Hauptfenster).
    pub(super) fn attention_viewport(&self, session: u64) -> egui::ViewportId {
        if self.detached.contains(&session) {
            return egui::ViewportId::from_hash_of(("detached", session));
        }
        viewport_of(self.window_of(session).unwrap_or(0))
    }

    /// Ist der Ticket-Verknüpfungsdialog für eine Sitzung dieses Fensters offen?
    pub(super) fn picker_here(&self) -> bool {
        self.tlinks_picker_session().is_none_or(|s| self.window_of(s).unwrap_or(0) == self.wins.cur)
    }

    /// Angesehene Sitzung über alle Fenster (für „gesehen“ und Benachrichtigungen): `(ein Fenster ist vorn, dessen fokussierte
    /// Sitzung)`. Nur mit dem Hauptfenster in den Feldern, nach `track_focus`.
    pub(super) fn seen_focus(&self) -> (bool, u64) {
        match self.wins.fg {
            None => (false, self.focused),
            Some(0) => (true, self.focused),
            Some(w) => (true, self.wins.list.iter().find(|x| x.id == w).map(|x| x.view.focused).unwrap_or(0)),
        }
    }

    /// Für `list_sessions`: (Fenster, im Fenster fokussiert, sichtbar). „Sichtbar“ wie bisher: im aktiven Projekt seines Fensters
    /// (Seitenleiste) oder abgedockt. Nur mit dem Hauptfenster in den Feldern.
    pub(super) fn session_view_info(&self, id: u64) -> (u64, bool, bool) {
        let Some(e) = self.entries.get(&id) else { return (0, false, false) };
        let w = e.meta.window;
        let detached = self.detached.contains(&id);
        if w == 0 {
            return (0, self.focused == id, detached || e.meta.project == self.active_project);
        }
        match self.wins.list.iter().find(|x| x.id == w) {
            Some(x) => (w, x.view.focused == id, detached || e.meta.project == x.view.active_project),
            None => (w, false, detached),
        }
    }

    /// Layout der gerade gezeichneten Ansicht bereinigen: nur Sitzungen dieses Fensters und Projekts, nichts Abgedocktes.
    fn prune_view(&mut self) {
        if !self.projects.iter().any(|p| p.id == self.active_project) {
            if let Some(first) = self.projects.first().map(|p| p.id) {
                self.active_project = first;
                let f = self.focused;
                self.load_project_view(None, f);
            }
        }
        let (pid, win) = (self.active_project, self.wins.cur);
        if let Some(l) = self.layout.take() {
            let entries = &self.entries;
            let detached = &self.detached;
            self.layout = prune_layout(l, |id| entries.get(&id).is_some_and(|e| e.meta.project == pid && e.meta.window == win) && !detached.contains(&id));
        }
        // Was im Layout steht, ist nicht minimiert – egal, wie es hineinkam (Ziehen, Ordner-Layout, Vorlage …).
        if let Some(l) = &self.layout {
            for id in l.leaf_ids() {
                if let Some(e) = self.entries.get_mut(&id).filter(|e| e.meta.minimized || e.meta.slot.is_some()) {
                    e.meta.minimized = false;
                    e.meta.slot = None;
                }
            }
        }
        let ok = self.layout.as_ref().is_some_and(|l| l.contains(self.focused));
        if !ok {
            let mut v = Vec::new();
            if let Some(l) = &self.layout {
                l.leaves(&mut v);
            }
            self.focused = v.first().copied().unwrap_or(0);
        }
    }

    fn enter(&mut self, w: &mut HubWindow) {
        swap_view(self, &mut w.view);
        self.wins.cur = w.id;
        style::set_id_salt(w.id); // Animationen (Hover, Fokus) je Fenster getrennt
        if w.data_gen != self.wins.data_gen {
            w.data_gen = self.wins.data_gen;
            self.plan.invalidate();
            self.tasks.invalidate();
        }
        if w.needs_load {
            w.needs_load = false;
            let (l, f) = (self.layout.take(), self.focused);
            self.load_project_view(l, f);
        }
        self.prune_view();
    }

    fn leave(&mut self, w: &mut HubWindow) {
        swap_view(self, &mut w.view);
        self.wins.cur = 0;
        style::set_id_salt(0);
    }

    /// Neuer Datenstand (`data_gen`), wenn seit dem letzten Aufruf in die Datenbank geschrieben wurde. Billig (ein Zähler
    /// der Verbindung) – ersetzt das Neuladen nach jeder Eingabe.
    fn note_data_changes(&mut self) -> bool {
        let total = self.db.as_ref().map(|d| d.conn.total_changes()).unwrap_or(0);
        let changed = data_changed(&mut self.wins.db_changes, total);
        if changed {
            self.wins.data_gen += 1;
        }
        changed
    }

    /// Was mit den Sitzungen von Fenster `window` beim Schließen geschieht.
    fn window_close_plan(&self, window: u64) -> ClosePlan {
        // „Läuft“: Terminal mit Prozess oder Chat mit laufender Antwort.
        let mut plan = close_plan(self.entries.iter().filter(|(_, e)| e.meta.window == window).map(|(id, e)| (*id, e.running(), e.meta.worktree.is_some(), self.detached.contains(id))));
        // Chats mit Verlauf: der Verlauf würde gelöscht → immer nachfragen.
        plan.chats = plan.end.iter().filter(|id| self.entries.get(id).is_some_and(|e| e.chat_has_history())).count();
        plan
    }

    /// Weitere Fenster anlegen, zeichnen und schließen. Im Hauptfenster nach dessen Aktionen aufrufen.
    pub(super) fn hub_windows(&mut self, ctx: &egui::Context) {
        debug_assert_eq!(self.wins.cur, 0);
        if !self.wins.is_empty() && self.note_data_changes() {
            self.wins.main_gen = self.wins.data_gen; // das Hauptfenster hat die Änderung selbst gemacht
        }
        if let Some(req) = self.wins.open_req.take() {
            self.open_window(req);
        }
        let mut list = std::mem::take(&mut self.wins.list);
        for w in list.iter_mut() {
            self.enter(w);
            let title = if w.hwnd.is_none() && w.hwnd_tries < 120 { format!("Paneldeck #{}", w.id) } else if self.last_title.is_empty() { "Paneldeck".to_string() } else { self.last_title.clone() };
            let mut b = egui::ViewportBuilder::default()
                .with_title(title.clone())
                .with_inner_size(w.init_size)
                .with_min_inner_size([640.0, 400.0])
                .with_transparent(true)
                .with_decorations(false)
                .with_icon(icon());
            if let Some(p) = w.init_pos {
                b = b.with_position(p);
            }
            if w.init_max {
                b = b.with_maximized(true);
            }
            if w.hidden && !self.in_tray && !self.start_in_tray {
                w.hidden = false; // Hauptfenster ist wieder da: weitere Fenster zeigen
            }
            b = b.with_active(!w.restored).with_visible(!w.hidden);
            ctx.show_viewport_immediate(w.viewport(), b, |ctx, class| {
                if class == egui::ViewportClass::Embedded {
                    return; // ohne echte Fenster (nur in Tests/anderen Oberflächen) gibt es keine weiteren Fenster
                }
                ctx.memory_mut(|m| std::mem::swap(&mut m.data, &mut w.mem));
                self.child_frame(ctx, w);
                ctx.memory_mut(|m| std::mem::swap(&mut m.data, &mut w.mem));
            });
            if w.hwnd.is_none() && w.hwnd_tries < 120 {
                w.hwnd_tries += 1;
                if let Some(h) = crate::winctl::find_own_window(&title) {
                    w.hwnd = Some(h);
                    winchrome::apply_corners(h);
                }
            }
            if w.close {
                let plan = self.window_close_plan(w.id);
                // Abgedocktes bleibt offen; Worktree-Sitzungen (sollte es nach der Rückfrage keine mehr geben) gehen nie still
                // verloren: beide wechseln ins Hauptfenster.
                for id in plan.adopt.iter().chain(&plan.worktrees) {
                    if let Some(e) = self.entries.get_mut(id) {
                        e.meta.window = 0;
                    }
                }
                for id in plan.end {
                    self.close_session(id);
                }
            }
            self.leave(w);
        }
        list.retain(|w| !w.close);
        drop_closed(&mut self.wins.queue, |w| list.iter().any(|x| x.id == w));
        if drop_closed(&mut self.wins.undo, |w| list.iter().any(|x| x.id == w)) > 0 {
            let lang = self.settings.language;
            self.push_toast(lang.t("Rückgängig nicht mehr möglich: das Fenster der Änderung wurde geschlossen.", "Undo is no longer possible: the window of that change was closed.").to_string(), super::toasts::ToastKind::Error, None);
        }
        // Während des Zeichnens neu angelegte Fenster (Strg+Klick im weiteren Fenster) anhängen.
        list.append(&mut self.wins.list);
        self.wins.list = list;
        // Akzent und Stil des Hauptfensters wiederherstellen (aus dem Zwischenspeicher, kein neues `style::apply`).
        self.sync_accent();
        self.ensure_style(ctx);
        if self.wins.exit_after_save {
            self.save_all();
            std::process::exit(0);
        }
        if let Some(req) = self.wins.open_req.take() {
            self.open_window(req);
        }
    }

    fn open_window(&mut self, req: OpenReq) {
        self.wins.next = self.wins.next.max(1);
        let id = self.wins.next;
        self.wins.next += 1;
        let project = if self.projects.iter().any(|p| p.id == req.project) { req.project } else { self.projects.first().map(|p| p.id).unwrap_or(0) };
        let pos = req.from.map(|r| r.min + Vec2::splat(32.0));
        let size = req.size.unwrap_or(Vec2::new(1100.0, 720.0)).max(Vec2::new(640.0, 400.0));
        let mut w = HubWindow::new(id, WinView::new(req.mode, project), pos, size, false);
        w.needs_load = false; // leer, nichts zu prüfen
        w.data_gen = self.wins.data_gen;
        self.wins.list.push(w);
    }

    /// Ein Bild eines weiteren Fensters (die Ansicht ist schon eingetauscht).
    fn child_frame(&mut self, ctx: &egui::Context, w: &mut HubWindow) {
        // Eigene Akzentfarbe (Projektfarbe der Todo-Ansicht dieses Fensters); der Stil kommt aus dem Zwischenspeicher.
        self.sync_accent();
        self.ensure_style(ctx);
        let (close_req, focused, minimized, maximized, outer, inner) = ctx.input(|i| {
            let v = i.viewport();
            (v.close_requested(), v.focused.unwrap_or(false), v.minimized.unwrap_or(false), v.maximized.unwrap_or(false), v.outer_rect, v.inner_rect)
        });
        w.os_focused = focused;
        w.maximized = maximized;
        if !minimized && !maximized {
            w.outer = outer.or(w.outer);
            w.inner = inner.or(w.inner);
        }
        if close_req && !w.close {
            if self.window_close_plan(w.id).ask() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                w.ask_close = true;
            } else {
                w.close = true;
            }
        }
        self.run_routed_undo(ctx);
        let mut actions = self.take_routed(ctx);
        self.poll_history_hits();
        self.handle_drops(ctx);
        self.titlebar(ctx);
        self.window_edges(ctx);
        self.mode_shortcuts(ctx);
        let tasks = self.mode == titlebar::Mode::Tasks;
        if !tasks {
            self.shortcuts(ctx, &mut actions);
            self.sidebar(ctx, &mut actions);
            self.files_ui(ctx, &mut actions);
            self.central(ctx, &mut actions);
            self.handle_drag(ctx);
        } else {
            self.tasks_view(ctx);
        }
        if self.overlay_window() == w.id {
            self.notice_ui(ctx);
            self.toasts_ui(ctx);
        }
        self.text_prompt_ui(ctx, &mut actions);
        self.dialogs(ctx, &mut actions);
        if self.picker_here() {
            self.tlinks_picker_ui(ctx);
        }
        self.restore_dialog(ctx);
        if w.ask_close {
            self.close_window_dialog(ctx, w, &mut actions);
        }
        for a in actions {
            self.apply(ctx, a);
        }
        let title = self.view_title(w.id);
        if title != self.last_title {
            self.last_title = title;
        }
        if self.note_data_changes() {
            w.data_gen = self.wins.data_gen;
        }
        let animating = self.entries.values().any(|e| e.meta.window == w.id && e.status == ClaudeStatus::Working && !e.exited());
        if animating {
            ctx.request_repaint_after(Duration::from_millis(80));
        }
    }

    /// Fenstertitel der gerade gezeichneten Ansicht (weitere Fenster tragen ihre Nummer).
    fn view_title(&self, window: u64) -> String {
        let waiting = self.entries.values().filter(|e| e.needs_you()).count();
        let prefix = if waiting > 0 { format!("({waiting}) ") } else { String::new() };
        let base = match self.entries.get(&self.focused) {
            Some(e) if self.mode == titlebar::Mode::Terminals => format!("{prefix}{} – Paneldeck", e.display_name()),
            _ => format!("{prefix}Paneldeck"),
        };
        if window == 0 { base } else { format!("{base} ({})", window + 1) }
    }

    /// Kurzer Hinweis unten in der Mitte (`notify_text`).
    pub(super) fn notice_ui(&mut self, ctx: &egui::Context) {
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
    }

    /// Rückfrage beim Schließen eines weiteren Fensters: was beendet wird, was ins Hauptfenster wechselt und welche
    /// Worktree-Sitzungen erst einzeln geschlossen werden müssen (über die gewohnte Worktree-Rückfrage).
    fn close_window_dialog(&mut self, ctx: &egui::Context, w: &mut HubWindow, actions: &mut Vec<Action>) {
        let lang = self.settings.language;
        let plan = self.window_close_plan(w.id);
        if !plan.ask() {
            w.ask_close = false;
            w.close = true;
            return;
        }
        if self.wt_close.is_some() {
            return; // die Worktree-Rückfrage einer Sitzung ist offen; danach geht es hier weiter
        }
        let names = |ids: &[u64]| ids.iter().filter_map(|id| self.entries.get(id).map(|e| (*id, e.display_name()))).collect::<Vec<_>>();
        let (adopt, worktrees) = (names(&plan.adopt), names(&plan.worktrees));
        let n = plan.running;
        egui::Window::new(lang.t("Fenster schließen?", "Close window?"))
            .id(Id::new(("close_win_ask", w.id)))
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_max_width(420.0);
                if n == 1 {
                    ui.label(lang.t("In diesem Fenster läuft noch eine Sitzung. Beim Schließen wird sie beendet.", "One session is still running in this window. Closing the window will end it."));
                } else if n > 1 {
                    ui.label(if lang == Lang::De {
                        format!("In diesem Fenster laufen noch {n} Sitzungen. Beim Schließen werden sie beendet.")
                    } else {
                        format!("{n} sessions are still running in this window. Closing the window will end them.")
                    });
                }
                if plan.chats > 0 {
                    ui.label(if plan.chats == 1 {
                        lang.t("Der Verlauf eines Chats wird dabei gelöscht.", "The history of one chat will be deleted.").to_string()
                    } else if lang == Lang::De {
                        format!("Die Verläufe von {} Chats werden dabei gelöscht.", plan.chats)
                    } else {
                        format!("The histories of {} chats will be deleted.", plan.chats)
                    });
                }
                if !adopt.is_empty() {
                    ui.add_space(S1);
                    ui.label(lang.t("Abgedockte Sitzungen bleiben offen und gehören danach zum Hauptfenster:", "Detached sessions stay open and move to the main window:"));
                    for (_, name) in &adopt {
                        ui.label(RichText::new(format!("• {name}")).color(style::dim(ui)));
                    }
                }
                if !worktrees.is_empty() {
                    ui.add_space(S1);
                    ui.label(lang.t(
                        "Sitzungen mit Git-Worktree zuerst schließen (Worktree behalten, entfernen oder mergen):",
                        "Close the sessions with a Git worktree first (keep, remove or merge the worktree):",
                    ));
                    for (id, name) in &worktrees {
                        ui.horizontal(|ui| {
                            ui.label(format!("• {name}"));
                            if ui.small_button(lang.t("Schließen …", "Close …")).clicked() {
                                actions.push(Action::RequestClose(*id));
                            }
                        });
                    }
                }
                ui.add_space(S2);
                ui.horizontal(|ui| {
                    let close = ui
                        .add_enabled(plan.can_close(), egui::Button::new(lang.t("Fenster schließen", "Close window")))
                        .on_disabled_hover_text(lang.t("Erst die Sitzungen mit Worktree schließen.", "Close the sessions with a worktree first."));
                    if close.clicked() {
                        w.ask_close = false;
                        w.close = true;
                    }
                    if ui.button(lang.t("Abbrechen", "Cancel")).clicked() {
                        w.ask_close = false;
                    }
                });
            });
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            w.ask_close = false;
        }
    }

    /// Weitere Fenster für `state.json`. Nur mit dem Hauptfenster in den Feldern aufrufen.
    pub(super) fn saved_windows(&self) -> Vec<SavedWindow> {
        self.wins
            .list
            .iter()
            .map(|w| SavedWindow {
                id: w.id,
                tab: mode_key(w.view.mode).to_string(),
                project: w.view.active_project,
                layout: w.view.layout.clone(),
                focused: w.view.focused,
                views: w.view.proj_views.iter().filter(|(p, _, _)| *p != w.view.active_project).map(|(p, l, f)| SavedProjectView { project: *p, layout: l.clone(), focused: *f }).collect(),
                pos: w.outer.or(w.init_pos.map(|p| Rect::from_min_size(p, Vec2::ZERO))).map(|r| [r.min.x, r.min.y]),
                size: Some(w.inner.map(|r| r.size()).unwrap_or(w.init_size)).map(|s| [s.x, s.y]),
                maximized: w.maximized,
            })
            .collect()
    }

    /// Weitere Fenster aus `state.json` wiederherstellen (nach dem Laden der Sitzungen und Projekte, vor der Ansicht des
    /// Hauptfensters). Sitzungen ohne gültiges Fenster gehen ans Hauptfenster.
    /// `ppp`: physische Pixel je Punkt (für den Vergleich der gespeicherten Lage mit den Monitoren).
    pub(super) fn restore_windows(&mut self, saved: &[SavedWindow], ppp: f32) {
        let first = self.projects.first().map(|p| p.id).unwrap_or(0);
        let plan = restorable(saved, |p| self.projects.iter().any(|x| x.id == p), first);
        let ids: Vec<u64> = plan.iter().map(|(s, _)| s.id).collect();
        for (s, project) in plan {
            let mut view = WinView::new(mode_from_key(&s.tab), project);
            view.layout = s.layout.clone();
            view.focused = s.focused;
            view.proj_views = s.views.iter().filter(|v| v.project != project).map(|v| (v.project, v.layout.clone(), v.focused)).collect();
            let pos = s.pos.map(|p| Pos2::new(p[0], p[1]));
            let size = s.size.map(|v| Vec2::new(v[0], v[1])).unwrap_or(Vec2::new(1100.0, 720.0));
            let (pos, size) = fit_window(pos, size, ppp, crate::winctl::point_on_screen, crate::winctl::work_area);
            let mut w = HubWindow::new(s.id, view, pos, size, s.maximized);
            // Beim Start nicht den Fokus vom Hauptfenster nehmen; startet Paneldeck im Tray, bleiben sie bis dahin verborgen.
            w.restored = true;
            w.hidden = self.start_in_tray;
            self.wins.list.push(w);
        }
        self.wins.next = ids.iter().copied().max().unwrap_or(0) + 1;
        adopt_orphans(self.entries.values_mut().map(|e| &mut e.meta.window), &ids);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{SavedState, SessionMeta};

    fn leaves(n: &Option<Node>) -> Vec<u64> {
        let mut v = Vec::new();
        if let Some(n) = n {
            n.leaves(&mut v);
        }
        v
    }

    fn project(id: u64, layout: Option<Node>, focused: u64) -> Project {
        Project { id, layout, focused, ..Project::default() }
    }

    #[test]
    fn project_views_swap_back_and_forth() {
        // Hauptfenster: Projekt 1 hat Layout [1], Projekt 2 [2]; das weitere Fenster kennt nur Projekt 2 mit [7].
        let mut projects = vec![project(1, Some(Node::Leaf(1)), 1), project(2, Some(Node::Leaf(2)), 2)];
        let mut stash = vec![(2, Some(Node::Leaf(7)), 7), (99, Some(Node::Leaf(9)), 9)];
        swap_project_views(&mut projects, &mut stash);
        assert_eq!(leaves(&projects[0].layout), Vec::<u64>::new(), "Projekt 1 ist im weiteren Fenster leer");
        assert_eq!(leaves(&projects[1].layout), vec![7]);
        assert_eq!(projects[1].focused, 7);
        // Während das weitere Fenster aktiv ist, entsteht ein neues Projekt.
        projects.push(project(3, Some(Node::Leaf(30)), 30));
        swap_project_views(&mut projects, &mut stash);
        assert_eq!(leaves(&projects[0].layout), vec![1], "zurück: Hauptfenster hat seine Layouts wieder");
        assert_eq!(leaves(&projects[1].layout), vec![2]);
        assert_eq!(leaves(&projects[2].layout), Vec::<u64>::new(), "neues Projekt hat im Hauptfenster noch kein Layout");
        let in_stash: Vec<u64> = stash.iter().map(|s| s.0).collect();
        assert_eq!(in_stash, vec![1, 2, 3], "gelöschtes Projekt 99 fällt weg");
        assert_eq!(leaves(&stash[2].1), vec![30], "das weitere Fenster behält sein Layout des neuen Projekts");
    }

    /// Dieselbe Feldliste wie `swap_view`, aber gegen eine zweite Ansicht (eine `App` lässt sich im Test nicht bauen).
    fn swap_views(a: &mut WinView, v: &mut WinView) {
        view_fields!(swap_fields!(a, v, |pv| std::mem::swap(&mut a.proj_views, pv)));
    }

    fn marked(tag: u64) -> WinView {
        let mut v = WinView::new(if tag % 2 == 0 { titlebar::Mode::Tasks } else { titlebar::Mode::Terminals }, tag);
        v.layout = Some(Node::Leaf(tag * 10));
        v.focused = tag * 10;
        v.filter = format!("filter-{tag}");
        v.composer_text = format!("draft-{tag}");
        v.queue_draft = format!("queue-{tag}");
        v.history_filter = format!("hist-{tag}");
        v.settings_tab = tag as usize;
        v.capture_key = Some(format!("key-{tag}"));
        v.confirm_restore = Some(PathBuf::from(format!("backup-{tag}.zip")));
        v.tpl_name = format!("tpl-{tag}");
        v.proj_views = vec![(tag, Some(Node::Leaf(tag)), tag)];
        v
    }

    #[allow(clippy::type_complexity)]
    fn sig(v: &WinView) -> ((Vec<u64>, u64, u64, titlebar::Mode), (String, String, String, String), (usize, Option<String>, Option<PathBuf>, String, Vec<u64>)) {
        (
            (leaves(&v.layout), v.focused, v.active_project, v.mode),
            (v.filter.clone(), v.composer_text.clone(), v.queue_draft.clone(), v.history_filter.clone()),
            (v.settings_tab, v.capture_key.clone(), v.confirm_restore.clone(), v.tpl_name.clone(), v.proj_views.iter().map(|p| p.0).collect()),
        )
    }

    #[test]
    fn swapping_a_view_twice_is_identity() {
        let (mut a, mut b) = (marked(1), marked(2));
        let (sa, sb) = (sig(&a), sig(&b));
        swap_views(&mut a, &mut b);
        assert_eq!(sig(&a), sb, "einmal getauscht: die App zeigt das andere Fenster");
        assert_eq!(sig(&b), sa);
        swap_views(&mut a, &mut b);
        assert_eq!(sig(&a), sa, "zweimal getauscht: alles wie vorher");
        assert_eq!(sig(&b), sb);
    }

    #[test]
    fn prune_removes_foreign_and_missing_sessions() {
        let l = Node::grid(&[1, 2, 3, 4], Dir::Horizontal).unwrap();
        let kept = prune_layout(l, |id| id % 2 == 0);
        assert_eq!(leaves(&kept), vec![2, 4]);
        assert!(prune_layout(Node::Leaf(5), |_| false).is_none(), "nichts übrig → kein Layout (leerer Hub)");
        // doppelte Blätter (dieselbe Sitzung zweimal) und unsinnige Anteile werden bereinigt
        let dup = Node::Split { dir: Dir::Vertical, ratio: f32::NAN, a: Box::new(Node::Leaf(2)), b: Box::new(Node::grid(&[2, 4], Dir::Horizontal).unwrap()) };
        let kept = prune_layout(dup, |_| true).unwrap();
        assert_eq!(kept.leaf_ids(), vec![2, 4]);
        assert!(kept.is_normal(|_| true));
        // schon in Ordnung → derselbe Baum
        let ok = Node::grid(&[1, 2, 3], Dir::Horizontal).unwrap();
        assert_eq!(prune_layout(ok.clone(), |_| true), Some(ok));
    }

    #[test]
    fn exactly_one_foreground_window() {
        assert_eq!(foreground(true, [(2, true)].into_iter()), Some(0), "Hauptfenster vorn (aktueller Stand) gewinnt");
        assert_eq!(foreground(false, [(2, false), (3, true)].into_iter()), Some(3));
        assert_eq!(foreground(false, [(2, false)].into_iter()), None, "kein Fenster vorn: nichts gilt als gesehen, Toasts im Hauptfenster");
        assert_eq!(foreground(false, [(2, true), (3, true)].into_iter()), Some(2), "höchstens eins, auch bei veraltetem Stand");
    }

    #[test]
    fn routed_entries_of_closed_windows_are_dropped() {
        let mut q = vec![(0u64, 'a'), (2, 'b'), (5, 'c'), (2, 'd')];
        assert_eq!(drop_closed(&mut q, |w| w == 2), 1, "Fenster 5 ist zu");
        assert_eq!(q, vec![(0, 'a'), (2, 'b'), (2, 'd')], "das Hauptfenster gibt es immer");
        assert_eq!(drop_closed(&mut q, |_| false), 2);
        assert_eq!(q, vec![(0, 'a')]);
    }

    #[test]
    fn actions_for_sessions_of_other_windows_are_routed() {
        let window = |id: u64| match id {
            1 => Some(0),
            2 => Some(3),
            _ => None,
        };
        let detached = |id: u64| id == 9;
        assert_eq!(route_target(&Action::Activate(2), window, detached, 0), Some(3));
        assert_eq!(route_target(&Action::Focus(2), window, detached, 0), Some(3));
        assert_eq!(route_target(&Action::JumpTo(1), window, detached, 3), Some(0));
        assert_eq!(route_target(&Action::GlobalJump(2, 4, "x".into()), window, detached, 0), Some(3));
        assert_eq!(route_target(&Action::Attach(2), window, detached, 0), Some(3));
        assert_eq!(route_target(&Action::Activate(2), window, detached, 3), None, "eigenes Fenster: hier ausführen");
        assert_eq!(route_target(&Action::Activate(42), window, detached, 0), None, "unbekannte Sitzung");
        assert_eq!(route_target(&Action::NewQuick, window, detached, 0), None);
        let det_other = |id: u64| if id == 9 { Some(3) } else { None };
        assert_eq!(route_target(&Action::Activate(9), det_other, detached, 0), None, "abgedockt: das Fenster der Sitzung holt jeder nach vorn");
        assert_eq!(route_target(&Action::Attach(9), det_other, detached, 0), Some(3), "Anheften gehört ins Fenster der Sitzung");
    }

    #[test]
    fn new_sessions_land_in_the_wanted_window_while_it_exists() {
        let exists = |w: u64| w == 3;
        assert_eq!(pick_window(None, 0, exists), 0, "ohne Wunsch: das gezeichnete Fenster");
        assert_eq!(pick_window(Some(3), 0, exists), 3, "Worktree aus Fenster 3, fertig im Hauptbild → Fenster 3");
        assert_eq!(pick_window(Some(0), 3, exists), 0);
        assert_eq!(pick_window(Some(5), 0, exists), 0, "inzwischen geschlossen: das gezeichnete Fenster");
        assert_eq!(pick_window(Some(3), 3, |_| false), 3, "das gezeichnete Fenster steht nicht in der Liste, gilt aber");
    }

    #[test]
    fn agent_children_follow_the_callers_window_and_project() {
        assert_eq!(child_home(Some((2, 7)), 1), (2, 7), "Agent in Fenster 2, Projekt 7");
        assert_eq!(child_home(None, 1), (0, 1), "Agent außerhalb des Hubs: Hauptfenster, dessen Projekt");
    }

    #[test]
    fn closing_a_window_ends_plain_sessions_keeps_detached_and_worktrees() {
        // (Id, läuft, Worktree, abgedockt)
        let p = close_plan([(1, true, false, false), (2, false, false, false), (3, true, false, true), (4, false, true, false), (5, true, true, true)].into_iter());
        assert_eq!(p.end, vec![1, 2]);
        assert_eq!(p.running, 1, "nur laufende zählen für den Text");
        assert_eq!(p.adopt, vec![3], "abgedockt: bleibt offen, wechselt ins Hauptfenster");
        assert_eq!(p.worktrees, vec![4, 5], "Worktree geht vor abgedockt: erst über die Worktree-Rückfrage");
        assert!(p.ask() && !p.can_close());
        let quiet = close_plan([(1, false, false, false)].into_iter());
        assert!(!quiet.ask() && quiet.can_close(), "nur beendete Sitzungen: ohne Rückfrage schließen");
        let running = close_plan([(1, true, false, false)].into_iter());
        assert!(running.ask(), "laufende Sitzung: immer fragen, auch wenn „Schließen bestätigen“ aus ist");
        assert!(close_plan(std::iter::empty()).can_close());
        let chats = ClosePlan { end: vec![1], chats: 1, ..ClosePlan::default() };
        assert!(chats.ask(), "Chat mit Verlauf: nachfragen, auch wenn er gerade nicht antwortet");
    }

    #[test]
    fn restored_window_positions_fit_the_monitors() {
        // Ein Monitor 1920×1080 Pixel, Arbeitsfläche ohne 40 px Taskleiste; Skalierung 150 %.
        let on = |x: f32, y: f32| (0.0..1920.0).contains(&x) && (0.0..1080.0).contains(&y);
        let work = |_: f32, _: f32| Some([0.0, 0.0, 1920.0, 1040.0]);
        let (pos, size) = fit_window(Some(Pos2::new(100.0, 80.0)), Vec2::new(900.0, 600.0), 1.5, on, work);
        assert_eq!(pos, Some(Pos2::new(100.0, 80.0)), "Punkte × 1,5 = Pixel liegen auf dem Monitor");
        assert_eq!(size, Vec2::new(900.0, 600.0));
        let (pos, _) = fit_window(Some(Pos2::new(1500.0, 80.0)), Vec2::new(900.0, 600.0), 1.5, on, work);
        assert_eq!(pos, None, "1500 Punkte = 2250 Pixel: außerhalb, das System wählt die Lage");
        let (pos, size) = fit_window(Some(Pos2::new(1000.0, 500.0)), Vec2::new(3000.0, 2000.0), 1.5, on, work);
        assert!((size.x - 1280.0).abs() < 0.01 && (size.y - 1040.0 / 1.5).abs() < 0.01, "höchstens die Arbeitsfläche (in Punkten): {size:?}");
        assert_eq!(pos, Some(Pos2::new(0.0, 0.0)), "so verschoben, dass das Fenster ganz darauf liegt");
        let (_, size) = fit_window(None, Vec2::new(100.0, 100.0), 1.0, on, work);
        assert_eq!(size, MIN_SIZE, "nie kleiner als die Mindestgröße");
        let (pos, size) = fit_window(Some(Pos2::new(f32::NAN, 0.0)), Vec2::new(f32::INFINITY, 1.0), 0.0, on, |_, _| None);
        assert_eq!((pos, size), (None, Vec2::new(1100.0, 720.0)), "kaputte Werte: Standard");
    }

    #[test]
    fn restore_skips_main_and_duplicate_ids_and_replaces_deleted_projects() {
        let w = |id: u64, project: u64| SavedWindow { id, project, ..SavedWindow::default() };
        let saved = vec![w(0, 1), w(2, 1), w(2, 3), w(3, 9), w(4, 3)];
        let plan = restorable(&saved, |p| p == 1 || p == 3, 1);
        let got: Vec<(u64, u64)> = plan.iter().map(|(s, p)| (s.id, *p)).collect();
        assert_eq!(got, vec![(2, 1), (3, 1), (4, 3)], "0 = Hauptfenster, doppelte 2 einmal, Projekt 9 gibt es nicht mehr");
    }

    #[test]
    fn only_database_writes_count_as_data_changes() {
        let conn = crate::db::open_memory();
        let mut last = conn.total_changes();
        assert!(!data_changed(&mut last, conn.total_changes()), "nichts geschrieben: kein Neuladen");
        let _ = conn.query_row("SELECT count(*) FROM meta", [], |r| r.get::<_, i64>(0)).unwrap();
        assert!(!data_changed(&mut last, conn.total_changes()), "Lesen zählt nicht");
        conn.execute("INSERT INTO meta (key, value) VALUES ('x', '1')", []).unwrap();
        assert!(data_changed(&mut last, conn.total_changes()), "Schreiben zählt");
        assert!(!data_changed(&mut last, conn.total_changes()), "nur einmal");
    }

    #[test]
    fn ctrl_click_opens_a_window_plain_click_switches() {
        assert!(opens_window(Modifiers::COMMAND));
        assert!(opens_window(Modifiers::CTRL | Modifiers::COMMAND));
        assert!(!opens_window(Modifiers::NONE));
        assert!(!opens_window(Modifiers::COMMAND | Modifiers::SHIFT));
    }

    #[test]
    fn mode_keys_round_trip_and_unknown_is_terminals() {
        for m in [titlebar::Mode::Terminals, titlebar::Mode::Tasks] {
            assert_eq!(mode_from_key(mode_key(m)), m);
        }
        assert_eq!(mode_from_key(""), titlebar::Mode::Terminals);
        assert_eq!(mode_from_key("editor"), titlebar::Mode::Terminals);
    }

    #[test]
    fn orphaned_sessions_return_to_the_main_window() {
        let mut w = [0u64, 2, 5, 2];
        adopt_orphans(w.iter_mut(), &[2]);
        assert_eq!(w, [0, 2, 0, 2]);
    }

    #[test]
    fn new_window_view_starts_empty_with_the_clicked_tab() {
        let v = WinView::new(titlebar::Mode::Tasks, 4);
        assert!(v.layout.is_none() && v.focused == 0, "neues Fenster ohne Sitzung (#111)");
        assert_eq!(v.mode, titlebar::Mode::Tasks);
        assert_eq!(v.active_project, 4);
        assert!(v.proj_views.is_empty());
    }

    #[test]
    fn state_without_windows_loads_and_stays_compact() {
        // Alter Stand (vor 2.1.0): keine Fenster, kein `window`-Feld.
        let dir = std::env::temp_dir().join(format!("paneldeck-test-multiwin-old-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("state.json");
        std::fs::write(&p, r#"{"sessions":[{"id":3,"name":"pwsh","project":1}],"next_id":4,"projects":[{"id":1,"name":"Standard"}],"active_project":1}"#).unwrap();
        let (st, note) = crate::state::load_saved(&p);
        assert!(note.is_none());
        assert_eq!(st.sessions.len(), 1);
        assert_eq!(st.sessions[0].window, 0);
        assert!(st.extra_windows.is_empty());
        // Ohne weitere Fenster schreibt Paneldeck weder `extra_windows` noch `window` (ältere Versionen sehen dieselbe Datei).
        let v = serde_json::to_value(&st).unwrap();
        assert!(v.get("extra_windows").is_none());
        assert!(v["sessions"][0].get("window").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn state_with_windows_round_trips() {
        let dir = std::env::temp_dir().join(format!("paneldeck-test-multiwin-new-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("state.json");
        let st = SavedState {
            sessions: vec![SessionMeta { id: 3, project: 1, ..SessionMeta::default() }, SessionMeta { id: 4, project: 1, window: 2, ..SessionMeta::default() }],
            next_id: 5,
            projects: vec![Project { id: 1, ..Project::default() }],
            active_project: 1,
            extra_windows: vec![SavedWindow {
                id: 2,
                tab: "tasks".into(),
                project: 1,
                layout: Some(Node::Leaf(4)),
                focused: 4,
                views: vec![SavedProjectView { project: 7, layout: None, focused: 0 }],
                pos: Some([100.0, 80.0]),
                size: Some([900.0, 600.0]),
                maximized: true,
            }],
            ..SavedState::default()
        };
        crate::state::save_json(&p, &st);
        let (back, note) = crate::state::load_saved(&p);
        assert!(note.is_none());
        assert_eq!(back.sessions.iter().map(|s| (s.id, s.window)).collect::<Vec<_>>(), vec![(3, 0), (4, 2)]);
        assert_eq!(back.extra_windows.len(), 1);
        let w = &back.extra_windows[0];
        assert_eq!((w.id, w.tab.as_str(), w.project, w.focused, w.maximized), (2, "tasks", 1, 4, true));
        assert_eq!(leaves(&w.layout), vec![4]);
        assert_eq!(w.pos, Some([100.0, 80.0]));
        assert_eq!(w.size, Some([900.0, 600.0]));
        assert_eq!(w.views.len(), 1);
        // Eine ältere Version (ohne die neuen Felder) liest die Datei weiter: unbekannte Felder werden ignoriert.
        #[derive(serde::Deserialize)]
        #[allow(dead_code)]
        struct OldMeta {
            id: u64,
            project: u64,
        }
        #[derive(serde::Deserialize)]
        #[allow(dead_code)]
        struct OldState {
            sessions: Vec<OldMeta>,
            next_id: u64,
        }
        let text = std::fs::read_to_string(&p).unwrap();
        let old: OldState = serde_json::from_str(crate::state::strip_bom(&text)).unwrap();
        assert_eq!(old.sessions.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
