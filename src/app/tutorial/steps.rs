//! Die Schritte des Tutorials (#143) als Tabelle: kurze Tour und Kapitel. Jeder Schritt hat eine feste Kennung (Fortschritt
//! in `tutorial.json`), die Version, mit der er dazukam (Tour „Was ist neu“), ein Ziel (`Spot`, `None` = Blase in der
//! Mitte), Titel und Text auf Deutsch und Englisch (`Txt`, angezeigt über `lang.t(de, en)`), Vorbereitungen (`Prep`, beim
//! Betreten angewendet, damit „Weiter“ immer zu einem sichtbaren Ziel führt), eine optionale Erkennung der Aufgabe
//! (`done`: Zustand beim Betreten → jetzt; dann geht der Schritt von selbst weiter) und optional eine Voraussetzung
//! (`Need`: fehlt Claude Code oder Ollama, erklärt der Schritt das mit Link und lässt sich überspringen).
//!
//! Platzhalter im Text: `{key:<id>}` = aktuelles Tastenkürzel (`keys::DEFAULTS`).
//!
//! Neue sichtbare Funktionen bekommen hier ihre Schritte (Regel 10 in CLAUDE.md); die Tests in `tests.rs` prüfen Ziel,
//! beide Sprachen, Versionen und Platzhalter.
//!
//! Editor-Reiter (#54, 3.0.0): Kapitel `editor` und der Schritt `basics.editor`; Schritte mit Text im Editor öffnen eine
//! eigene Beispieldatei der Tour (`Prep::EditorFile`, nie eine Datei des Nutzers).

use super::marks::Spot;

/// Text in beiden Sprachen.
#[derive(Clone, Copy, Debug)]
pub(super) struct Txt {
    pub de: &'static str,
    pub en: &'static str,
}

const fn tx(de: &'static str, en: &'static str) -> Txt {
    Txt { de, en }
}

/// Was die Tour vom Zustand der Anwendung sieht (pro Bild erhoben, nur während einer Tour).
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Probe {
    /// Sitzungen im Tutorial-Projekt.
    pub sessions: usize,
    /// Bereiche im Layout (nur, wenn das Tutorial-Projekt aktiv ist).
    pub panes: usize,
    /// Minimierte Sitzungen im Tutorial-Projekt.
    pub minimized: usize,
    /// Prüfsumme über eigene Namen und Farben der Tutorial-Sitzungen (Umbenennen/Farbe erkennen).
    pub names: u64,
    pub palette: bool,
    /// Offener Reiter der Einstellungen (`None` = geschlossen).
    pub settings: Option<usize>,
    /// Reiter „Todo-Liste“ aktiv.
    pub tasks: bool,
    pub expert: bool,
    /// To-dos (alle) und erledigte.
    pub todos: usize,
    pub todos_done: usize,
    /// Tickets im Experten-Projekt der Tour.
    pub tickets: usize,
    /// Reiter der Projektansicht (`planning_ui::PlanUi::tab_key`).
    pub plan_tab: &'static str,
    pub history: bool,
    pub prompts: bool,
    pub composer: bool,
    pub files: bool,
    /// Dialog „Neuer Chat“ offen.
    pub new_chat: bool,
    /// Reiter „Editor“ aktiv (#54).
    pub editor: bool,
    /// Offene Dokumente im Editor.
    pub editor_docs: usize,
    /// Teilungen des Editors.
    pub editor_groups: usize,
    /// Suchleiste des Editors offen.
    pub editor_find: bool,
    /// Cursor (Auswahlen) im aktiven Reiter des Editors.
    pub editor_cursors: usize,
}

/// Erkennung „Aufgabe erledigt“: Zustand beim Betreten des Schritts, Zustand jetzt.
pub(super) type Done = fn(&Probe, &Probe) -> bool;

/// Vorbereitung beim Betreten eines Schritts (nur, wenn der Zustand nicht schon passt).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Prep {
    /// Reiter „Terminal Hub“ zeigen.
    Terminals,
    /// Reiter „Todo-Liste“ zeigen.
    Tasks,
    /// Einstellungen schließen.
    NoSettings,
    /// Einstellungen mit diesem Reiter öffnen (Indizes wie in `settings.rs`).
    Settings(usize),
    /// Tutorial-Projekt aktiv.
    TutProject,
    /// Tutorial-Projekt aktiv und mindestens eine Sitzung darin (sonst ein Terminal anlegen).
    Session,
    /// Befehls-Palette offen bzw. geschlossen.
    Palette(bool),
    /// Fenster Verlauf, Prompt-Bibliothek, Composer schließen.
    NoTools,
    /// Expertenmodus an bzw. aus.
    Expert(bool),
    /// Expertenmodus wie vor der Tour.
    ExpertRestore,
    /// Experten-Projekt der Tour anlegen (falls nötig) und wählen.
    PlanProject,
    /// Reiter der Projektansicht (`tab_key`).
    PlanTab(&'static str),
    /// Reiter „Editor“ zeigen (#54).
    Editor,
    /// Ordnerbaum des Editors einblenden.
    EditorTree,
    /// Beispieldatei der Tour im Editor öffnen und zeigen (eine eigene Datei im Temp-Ordner, nie eine des Nutzers).
    EditorFile,
    /// Suchleiste des Editors offen (mit Ersetzen) bzw. geschlossen.
    EditorFind(bool),
    /// Fenster „In Dateien suchen“ offen bzw. geschlossen.
    EditorSearch(bool),
}

/// Voraussetzung eines Schritts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Need {
    Claude,
    Ollama,
}

impl Need {
    pub(super) fn link(self) -> &'static str {
        match self {
            Need::Claude => "https://docs.anthropic.com/en/docs/claude-code",
            Need::Ollama => "https://ollama.com/download",
        }
    }

    pub(super) fn note(self) -> Txt {
        match self {
            Need::Claude => tx(
                "Claude Code ist auf diesem Rechner nicht gefunden worden (nicht im PATH). Die Installation erklärt die offizielle Anleitung; du kannst diesen Schritt auch überspringen.",
                "Claude Code was not found on this computer (not in PATH). The official guide explains how to install it; you can also skip this step.",
            ),
            Need::Ollama => tx(
                "Ollama ist auf diesem Rechner nicht gefunden worden (nicht im PATH) und noch kein Server eingerichtet. Ollama gibt es kostenlos auf der offiziellen Seite; du kannst diesen Schritt auch überspringen.",
                "Ollama was not found on this computer (not in PATH) and no server is set up yet. Ollama is available for free on its official page; you can also skip this step.",
            ),
        }
    }
}

/// Ein Schritt.
pub(super) struct Step {
    pub id: &'static str,
    /// Version, mit der die gezeigte Funktion (und damit der Schritt) dazukam.
    pub since: &'static str,
    pub spot: Option<Spot>,
    pub title: Txt,
    pub body: Txt,
    pub prep: &'static [Prep],
    pub done: Option<Done>,
    pub need: Option<Need>,
}

/// Eine Tour (kurze Tour oder Kapitel).
pub(super) struct Tour {
    pub id: &'static str,
    pub title: Txt,
    pub summary: Txt,
    pub steps: &'static [Step],
}

// Erkennungen
fn sessions_grew(a: &Probe, b: &Probe) -> bool {
    b.sessions > a.sessions
}
fn names_changed(a: &Probe, b: &Probe) -> bool {
    b.names != a.names && b.sessions >= a.sessions
}
fn panes_grew(a: &Probe, b: &Probe) -> bool {
    b.panes > a.panes
}
fn minimized_grew(a: &Probe, b: &Probe) -> bool {
    b.minimized > a.minimized
}
fn palette_opened(a: &Probe, b: &Probe) -> bool {
    b.palette && !a.palette
}
fn tasks_shown(a: &Probe, b: &Probe) -> bool {
    b.tasks && !a.tasks
}
fn todos_grew(a: &Probe, b: &Probe) -> bool {
    b.todos > a.todos
}
fn todos_done_grew(a: &Probe, b: &Probe) -> bool {
    b.todos_done > a.todos_done
}
fn expert_toggled(a: &Probe, b: &Probe) -> bool {
    b.expert != a.expert
}
fn expert_on(a: &Probe, b: &Probe) -> bool {
    b.expert && !a.expert
}
fn tickets_grew(a: &Probe, b: &Probe) -> bool {
    b.tickets > a.tickets
}
fn boards_shown(a: &Probe, b: &Probe) -> bool {
    b.plan_tab == "boards" && a.plan_tab != "boards"
}
fn backlog_shown(a: &Probe, b: &Probe) -> bool {
    b.plan_tab == "backlog" && a.plan_tab != "backlog"
}
fn search_shown(a: &Probe, b: &Probe) -> bool {
    b.plan_tab == "search" && a.plan_tab != "search"
}
fn history_opened(a: &Probe, b: &Probe) -> bool {
    b.history && !a.history
}
fn prompts_opened(a: &Probe, b: &Probe) -> bool {
    b.prompts && !a.prompts
}
fn composer_opened(a: &Probe, b: &Probe) -> bool {
    b.composer && !a.composer
}
fn files_toggled(a: &Probe, b: &Probe) -> bool {
    b.files != a.files
}
fn chat_started(a: &Probe, b: &Probe) -> bool {
    (b.new_chat && !a.new_chat) || b.sessions > a.sessions
}
fn editor_shown(a: &Probe, b: &Probe) -> bool {
    b.editor && !a.editor
}
fn editor_docs_grew(a: &Probe, b: &Probe) -> bool {
    b.editor_docs > a.editor_docs
}
fn editor_split(a: &Probe, b: &Probe) -> bool {
    b.editor_groups > a.editor_groups
}
fn editor_find_opened(a: &Probe, b: &Probe) -> bool {
    b.editor_find && !a.editor_find
}
fn editor_cursor_added(a: &Probe, b: &Probe) -> bool {
    b.editor_cursors > 1 && b.editor_cursors > a.editor_cursors
}

// Indizes der Einstellungsreiter (siehe `settings.rs`).
const S_APPEARANCE: usize = 1;
const S_CLAUDE: usize = 2;
const S_SHORTCUTS: usize = 5;
const S_PROFILES: usize = 6;
const S_AGENTS: usize = 7;
const S_DATA: usize = 8;
const S_ABOUT: usize = 9;
const S_MODELS: usize = super::super::models_ui::SETTINGS_TAB;
const S_EDITOR: usize = super::super::editor_services::SETTINGS_TAB;

const HUB: &[Prep] = &[Prep::Terminals, Prep::NoSettings, Prep::Palette(false), Prep::NoTools, Prep::TutProject];
/// Editor mit der Beispieldatei der Tour, ohne Suchleiste und Suchfenster.
const EDITOR_FILE: &[Prep] = &[Prep::NoSettings, Prep::Palette(false), Prep::NoTools, Prep::EditorSearch(false), Prep::Editor, Prep::EditorFile, Prep::EditorFind(false)];
const HUB_SESSION: &[Prep] = &[Prep::Terminals, Prep::NoSettings, Prep::Palette(false), Prep::NoTools, Prep::Session];
const TODO_SIMPLE: &[Prep] = &[Prep::NoSettings, Prep::Tasks, Prep::Expert(false)];
const PLAN: &[Prep] = &[Prep::NoSettings, Prep::Tasks, Prep::Expert(true), Prep::PlanProject];

/// Die kurze Tour: zeigt alles grob.
const BASICS: &[Step] = &[
    Step {
        // Begrüßung der kurzen Tour selbst (keine eigene Funktion): gilt als älteste, damit „Was ist neu“ sie nicht zeigt.
        id: "basics.welcome",
        since: "1.0.0",
        spot: None,
        title: tx("Willkommen zur Tour", "Welcome to the tour"),
        body: tx(
            "Diese kurze Tour (etwa 3 Minuten) zeigt die wichtigsten Teile von Paneldeck. Sie läuft in einem eigenen Projekt „Tutorial“ mit eigenen Sitzungen – deine Sitzungen bleiben unberührt. Schritte mit einer Aufgabe gehen von selbst weiter, sobald du sie erledigt hast; „Weiter“ und „Überspringen“ gehen immer. Esc oder × beendet die Tour, dein Fortschritt bleibt erhalten.",
            "This short tour (about 3 minutes) shows the main parts of Paneldeck. It runs in its own project “Tutorial” with its own sessions – your sessions stay untouched. Steps with a task continue by themselves once you have done it; “Next” and “Skip” always work. Esc or × ends the tour, your progress is kept.",
        ),
        prep: HUB,
        done: None,
        need: None,
    },
    Step {
        id: "basics.sidebar",
        since: "1.0.0",
        spot: Some(Spot::Sidebar),
        title: tx("Die Seitenleiste", "The sidebar"),
        body: tx(
            "Links stehen die Sitzungen des aktuellen Projekts. Die Werkzeugleiste oben startet Sitzungen und öffnet Werkzeuge; unten liegen Prozesse und Vorlagen.",
            "On the left you see the sessions of the current project. The toolbar on top starts sessions and opens tools; processes and templates sit at the bottom.",
        ),
        prep: HUB,
        done: None,
        need: None,
    },
    Step {
        id: "basics.new_terminal",
        since: "1.0.0",
        spot: Some(Spot::SidebarNew),
        title: tx("Ein Terminal starten", "Start a terminal"),
        body: tx(
            "Klicke auf +, um „Neue Sitzung“ zu öffnen und ein Terminal zu starten – oder nimm „Neues Terminal“ in der Mitte bzw. {key:new}. Die Tour geht weiter, sobald das Terminal da ist.",
            "Click + to open “New session” and start a terminal – or use “New terminal” in the middle or {key:new}. The tour continues once the terminal is there.",
        ),
        prep: HUB,
        done: Some(sessions_grew),
        need: None,
    },
    Step {
        id: "basics.rename",
        since: "1.0.0",
        spot: Some(Spot::SessionRow),
        title: tx("Umbenennen und Farbe", "Rename and colour"),
        body: tx(
            "Ein Rechtsklick auf die Sitzung öffnet ihr Menü: „Umbenennen (F2)“, „Farbe“, Ordner, Tags und mehr. Gib dem Terminal einen Namen oder eine Farbe, um weiterzumachen.",
            "Right-click the session to open its menu: “Rename (F2)”, “Color”, folders, tags and more. Give the terminal a name or a colour to continue.",
        ),
        prep: HUB_SESSION,
        done: Some(names_changed),
        need: None,
    },
    Step {
        id: "basics.projects",
        since: "1.0.0",
        spot: Some(Spot::ProjectBar),
        title: tx("Projekte", "Projects"),
        body: tx(
            "Projekte halten Sitzungen auseinander, jedes mit eigenem Layout. Gerade bist du im Projekt „Tutorial“. + legt ein Projekt an, ein Rechtsklick benennt es um oder setzt Farbe und Standardordner; Strg+Umschalt+1…9 wechselt.",
            "Projects keep sessions apart, each with its own layout. Right now you are in the project “Tutorial”. + creates a project, a right-click renames it or sets its colour and default folder; Ctrl+Shift+1…9 switches.",
        ),
        prep: HUB_SESSION,
        done: None,
        need: None,
    },
    Step {
        id: "basics.split",
        since: "1.0.0",
        spot: Some(Spot::PaneSplit),
        title: tx("Bereich teilen", "Split the pane"),
        body: tx(
            "Dieser Knopf im Kopf des Bereichs teilt ihn nach rechts ({key:split_r}, darunter: {key:split_d}). Teile ihn einmal, um weiterzumachen.",
            "This button in the pane header splits the pane to the right ({key:split_r}, below: {key:split_d}). Split it once to continue.",
        ),
        prep: HUB_SESSION,
        done: Some(panes_grew),
        need: None,
    },
    Step {
        id: "basics.minimize",
        since: "2.2.0",
        spot: Some(Spot::PaneMinimize),
        title: tx("Bereich minimieren", "Minimize a pane"),
        body: tx(
            "Minimieren nimmt einen Bereich aus dem Layout ({key:hide}) – die Sitzung läuft weiter und kommt mit einem Klick in der Seitenleiste zurück. Probier es aus.",
            "Minimize takes a pane out of the layout ({key:hide}) – the session keeps running and comes back with a click in the sidebar. Try it.",
        ),
        prep: HUB_SESSION,
        done: Some(minimized_grew),
        need: None,
    },
    Step {
        id: "basics.windows",
        since: "2.1.0",
        spot: Some(Spot::TitlebarTabs),
        title: tx("Weitere Fenster", "More windows"),
        body: tx(
            "Strg+Klick auf einen Reiter hier oben (Terminal Hub, Todo-Liste, Editor) öffnet ihn in einem neuen Fenster. Auch eine Sitzung bekommt ein eigenes Fenster: Rechtsklick → „In eigenem Fenster öffnen“.",
            "Ctrl+click a tab up here (Terminal Hub, To-do list, Editor) to open it in a new window. A session can get its own window too: right-click it → “Open in own window”.",
        ),
        prep: HUB,
        done: None,
        need: None,
    },
    Step {
        id: "basics.editor",
        since: "3.0.0",
        spot: Some(Spot::TitlebarEditor),
        title: tx("Editor", "Editor"),
        body: tx(
            "Der Reiter Editor ({key:mode_editor}) ist ein eigener Texteditor: Reiter, Teilungen, Suchen und Ersetzen, mehrere Cursor, Suche in Dateien. Klick darauf und sieh dich um – das Kapitel „Der Editor“ zeigt mehr.",
            "The Editor tab ({key:mode_editor}) is a text editor of its own: tabs, splits, find and replace, multiple cursors, search in files. Click it and have a look – the chapter “The editor” shows more.",
        ),
        prep: HUB,
        done: Some(editor_shown),
        need: None,
    },
    Step {
        id: "basics.palette",
        since: "1.0.0",
        spot: Some(Spot::SidebarPalette),
        title: tx("Befehls-Palette", "Command palette"),
        body: tx(
            "Jeder Befehl steht in der Befehls-Palette: Klick auf die Lupe oder {key:palette}. Öffne sie, um weiterzumachen.",
            "Every command is in the command palette: click the magnifier or press {key:palette}. Open it to continue.",
        ),
        prep: HUB,
        done: Some(palette_opened),
        need: None,
    },
    Step {
        id: "basics.palette_use",
        since: "1.0.0",
        spot: Some(Spot::Palette),
        title: tx("Tippen zum Suchen", "Type to search"),
        body: tx(
            "Tippe ein paar Buchstaben – z. B. „teilen“ oder „Tutorial“ – und drücke Enter für den markierten Befehl. Esc schließt die Palette.",
            "Type a few letters – e.g. “split” or “tutorial” – and press Enter to run the highlighted command. Esc closes the palette.",
        ),
        prep: &[Prep::Terminals, Prep::NoSettings, Prep::NoTools, Prep::Palette(true)],
        done: None,
        need: None,
    },
    Step {
        id: "basics.shortcuts",
        since: "1.0.0",
        spot: Some(Spot::TabShortcuts),
        title: tx("Tastenkürzel", "Keyboard shortcuts"),
        body: tx(
            "Hier lassen sich alle Tastenkürzel ändern: Klick auf ein Kürzel und die neue Tastenkombination drücken.",
            "All keyboard shortcuts can be changed here: click a shortcut and press the new key combination.",
        ),
        prep: &[Prep::Terminals, Prep::Palette(false), Prep::NoTools, Prep::Settings(S_SHORTCUTS)],
        done: None,
        need: None,
    },
    Step {
        id: "basics.claude",
        since: "1.0.0",
        spot: Some(Spot::SidebarClaude),
        title: tx("Claude Code", "Claude Code"),
        body: tx(
            "Der Funken-Knopf startet eine Claude-Code-Sitzung mit Modell, Aufwand und Berechtigungsmodus deiner Wahl. Paneldeck zeigt ihren Status in der Seitenleiste und im Kopf des Bereichs. Ohne Claude Code lässt sich der Claude-Modus unter Einstellungen → Claude abschalten.",
            "The spark button starts a Claude Code session with the model, effort and permission mode of your choice. Paneldeck shows its status in the sidebar and in the pane header. Without Claude Code, Claude mode can be switched off under Settings → Claude.",
        ),
        prep: HUB,
        done: None,
        need: Some(Need::Claude),
    },
    Step {
        id: "basics.costs",
        since: "1.0.0",
        spot: Some(Spot::SidebarFooter),
        title: tx("Status und Kosten", "Status and costs"),
        body: tx(
            "Hier unten summiert Paneldeck Token und geschätzte Kosten aller Claude-Sitzungen (sobald es welche gibt); ein Klick öffnet die Kostenübersicht je Sitzung und Tag. Ein Tages- oder Sitzungsbudget stellst du unter Einstellungen → Claude ein.",
            "Down here Paneldeck sums up the tokens and estimated costs of all Claude sessions (once there are any); a click opens the cost overview per session and day. A daily or session budget is set under Settings → Claude.",
        ),
        prep: HUB,
        done: None,
        need: None,
    },
    Step {
        id: "basics.chat",
        since: "2.2.0",
        spot: Some(Spot::SidebarChat),
        title: tx("Chats mit lokalen Modellen", "Chats with local models"),
        body: tx(
            "Dieser Knopf öffnet einen Chat mit einem lokalen Modell (Ollama oder ein OpenAI-kompatibler Server). Wie man das einrichtet, zeigt das Kapitel „Chats mit lokalen Modellen“.",
            "This button opens a chat with a local model (Ollama or an OpenAI-compatible server). The chapter “Chats with local models” shows how to set it up.",
        ),
        prep: HUB,
        done: None,
        need: Some(Need::Ollama),
    },
    Step {
        id: "basics.todo",
        since: "2.0.0",
        spot: Some(Spot::TitlebarTodo),
        title: tx("Todo-Liste", "To-do list"),
        body: tx(
            "Dieser Reiter wechselt zur Todo-Liste ({key:mode_tasks}). Klicke darauf, um weiterzumachen.",
            "This tab switches to the to-do list ({key:mode_tasks}). Click it to continue.",
        ),
        prep: HUB,
        done: Some(tasks_shown),
        need: None,
    },
    Step {
        id: "basics.todo_add",
        since: "2.0.0",
        spot: Some(Spot::TodoInput),
        title: tx("Ein To-do anlegen", "Add a to-do"),
        body: tx(
            "Schreib hier ein To-do und drücke Enter. „Mehr …“ ergänzt Titel, Tags und Fälligkeit. In der Tour angelegte To-dos werden mit dem Tutorial-Projekt entfernt.",
            "Type a to-do here and press Enter. “More …” adds a title, tags and a due date. To-dos created during the tour are removed together with the tutorial project.",
        ),
        prep: TODO_SIMPLE,
        done: Some(todos_grew),
        need: None,
    },
    Step {
        id: "basics.todo_check",
        since: "2.0.0",
        spot: Some(Spot::TodoList),
        title: tx("Abhaken", "Tick it off"),
        body: tx(
            "Hake das To-do mit dem Haken auf der Karte ab. Ein Klick auf die Karte bearbeitet sie, Ziehen sortiert.",
            "Tick the to-do off with the check mark on its card. A click on the card edits it, dragging sorts it.",
        ),
        prep: TODO_SIMPLE,
        done: Some(todos_done_grew),
        need: None,
    },
    Step {
        id: "basics.expert",
        since: "2.0.0",
        spot: Some(Spot::ExpertToggle),
        title: tx("Expertenmodus", "Expert mode"),
        body: tx(
            "Der Expertenmodus macht aus der Liste Projekte mit Tickets, Boards, Sprints und einer Abfragesprache. Schalte ihn einmal um und sieh dich um – die Tour stellt ihn danach zurück. Mehr zeigt das Kapitel „Todo-Liste & Expertenmodus“.",
            "Expert mode turns the list into projects with tickets, boards, sprints and a query language. Switch it over once and have a look – the tour puts it back afterwards. The chapter “To-do list & expert mode” shows more.",
        ),
        prep: &[Prep::NoSettings, Prep::Tasks],
        done: Some(expert_toggled),
        need: None,
    },
    Step {
        id: "basics.mcp",
        since: "1.0.0",
        spot: Some(Spot::McpLevel),
        title: tx("Agenten-Schnittstelle (MCP)", "Agent interface (MCP)"),
        body: tx(
            "Agenten wie Claude Code können deine Sitzungen über MCP sehen und steuern. Standardmäßig ist das aus; hier wählst du „Nur lesen“ oder „Steuern“, weiter unten trägst du Paneldeck in Claude Code ein. Das Kapitel „Agenten-Schnittstelle (MCP)“ erklärt die Stufen.",
            "Agents such as Claude Code can see and control your sessions through MCP. It is off by default; choose “Read only” or “Control” here and register Paneldeck with Claude Code further down. The chapter “Agent interface (MCP)” explains the levels.",
        ),
        prep: &[Prep::ExpertRestore, Prep::Terminals, Prep::Palette(false), Prep::NoTools, Prep::Settings(S_AGENTS)],
        done: None,
        need: None,
    },
    Step {
        id: "basics.help",
        since: "3.0.0",
        spot: Some(Spot::AboutTutorial),
        title: tx("Tour und Kapitel", "Tour and chapters"),
        body: tx(
            "Das war's! Die Tour und die Kapitel zu einzelnen Themen findest du immer hier, in der Befehls-Palette („Tutorial“) und im leeren Terminal Hub. Zum Schluss entscheidest du, ob das Tutorial-Projekt bleibt.",
            "That's it! The tour and the chapters on single topics are always here, in the command palette (“Tutorial”) and in the empty Terminal Hub. At the end you decide whether the tutorial project is kept.",
        ),
        prep: &[Prep::Terminals, Prep::Settings(S_ABOUT)],
        done: None,
        need: None,
    },
];

/// Kapitel „Der Editor“ (#54, 3.0.0). Schritte mit Text arbeiten in der Beispieldatei der Tour (`Prep::EditorFile`).
const EDITOR: &[Step] = &[
    Step {
        id: "editor.intro",
        since: "3.0.0",
        spot: Some(Spot::TitlebarEditor),
        title: tx("Der Reiter Editor", "The Editor tab"),
        body: tx(
            "Der Editor ist ein schlichter Texteditor mit eigenem Ordnerbaum und eigenen Dateien – unabhängig von den Terminals. Öffne ihn mit einem Klick oder {key:mode_editor}, um weiterzumachen.",
            "The editor is a plain text editor with its own folder tree and its own files – independent of the terminals. Open it with a click or {key:mode_editor} to continue.",
        ),
        prep: HUB,
        done: Some(editor_shown),
        need: None,
    },
    Step {
        id: "editor.open",
        since: "3.0.0",
        spot: Some(Spot::EditorTree),
        title: tx("Dateien und Ordner öffnen", "Open files and folders"),
        body: tx(
            "„Ordner öffnen …“ (Strg+Umschalt+O) zeigt einen Ordner als Baum; ein Klick öffnet eine Datei in einem Reiter, Strg+O öffnet eine einzelne Datei, Strg+B blendet den Baum aus. Ungespeicherte Änderungen überstehen einen Neustart. Öffne eine Datei, um weiterzumachen – oder geh mit „Weiter“ zur Beispieldatei der Tour.",
            "“Open folder …” (Ctrl+Shift+O) shows a folder as a tree; a click opens a file in a tab, Ctrl+O opens a single file, Ctrl+B hides the tree. Unsaved changes survive a restart. Open a file to continue – or use “Next” to get the tour's sample file.",
        ),
        prep: &[Prep::NoSettings, Prep::Palette(false), Prep::NoTools, Prep::EditorSearch(false), Prep::Editor, Prep::EditorTree],
        done: Some(editor_docs_grew),
        need: None,
    },
    Step {
        id: "editor.tabs",
        since: "3.0.0",
        spot: Some(Spot::EditorTabBar),
        title: tx("Reiter", "Tabs"),
        body: tx(
            "Die Tour hat eine eigene Beispieldatei geöffnet. Jede Teilung hat ihre eigene Reiterleiste: Reiter lassen sich ziehen – auch in eine andere Teilung –, Mittelklick schließt, Strg+Tab wechselt. Ein Punkt zeigt ungespeicherte Änderungen; Strg+S speichert.",
            "The tour opened a sample file of its own. Every split has its own tab bar: drag tabs – also into another split –, middle-click closes, Ctrl+Tab switches. A dot shows unsaved changes; Ctrl+S saves.",
        ),
        prep: EDITOR_FILE,
        done: None,
        need: None,
    },
    Step {
        id: "editor.split",
        since: "3.0.0",
        spot: Some(Spot::EditorSplit),
        title: tx("Teilen", "Split"),
        body: tx(
            "Teile den Editor nach rechts ({key:split_r}) oder nach unten ({key:split_d}), wie im Terminal Hub; Strg+Alt+Pfeil wechselt die Teilung. Dieselbe Datei in zwei Teilungen bleibt gleich, jede mit eigenem Cursor. Teile ihn einmal, um weiterzumachen.",
            "Split the editor to the right ({key:split_r}) or below ({key:split_d}), like in the Terminal Hub; Ctrl+Alt+arrow moves between splits. The same file in two splits stays in sync, each with its own cursor. Split it once to continue.",
        ),
        prep: EDITOR_FILE,
        done: Some(editor_split),
        need: None,
    },
    Step {
        id: "editor.find",
        since: "3.0.0",
        spot: Some(Spot::EditorText),
        title: tx("Suchen", "Find"),
        body: tx(
            "Klick in den Text und drücke Strg+F zum Suchen oder Strg+H zum Ersetzen. Öffne die Suche, um weiterzumachen.",
            "Click into the text and press Ctrl+F to find or Ctrl+H to replace. Open the search to continue.",
        ),
        prep: EDITOR_FILE,
        done: Some(editor_find_opened),
        need: None,
    },
    Step {
        id: "editor.replace",
        since: "3.0.0",
        spot: Some(Spot::EditorFind),
        title: tx("Suchen und Ersetzen", "Find and replace"),
        body: tx(
            "Aa beachtet Groß-/Kleinschreibung, W sucht ganze Wörter, .* nimmt reguläre Ausdrücke (beim Ersetzen mit $1 für Gruppen). Enter springt zum nächsten Treffer, „Alle ersetzen“ ist ein Schritt zum Rückgängigmachen. Esc schließt die Leiste.",
            "Aa matches case, W finds whole words, .* takes regular expressions (with $1 for groups when replacing). Enter jumps to the next match, “Replace all” is one step to undo. Esc closes the bar.",
        ),
        prep: &[Prep::NoSettings, Prep::Palette(false), Prep::NoTools, Prep::EditorSearch(false), Prep::Editor, Prep::EditorFile, Prep::EditorFind(true)],
        done: None,
        need: None,
    },
    Step {
        id: "editor.cursors",
        since: "3.0.0",
        spot: Some(Spot::EditorText),
        title: tx("Mehrere Cursor", "Multiple cursors"),
        body: tx(
            "Alt+Klick setzt einen weiteren Cursor, Alt+Umschalt+↑/↓ einen darüber oder darunter, Strg+D markiert das nächste gleiche Wort, Alt+Ziehen eine Spalte – alles, was du tippst, geht an alle. Esc lässt nur einen übrig. Setz einen zweiten Cursor, um weiterzumachen.",
            "Alt+click adds a cursor, Alt+Shift+Up/Down adds one above or below, Ctrl+D selects the next match of the word, Alt+drag selects a column – everything you type goes to all of them. Esc keeps only one. Add a second cursor to continue.",
        ),
        prep: EDITOR_FILE,
        done: Some(editor_cursor_added),
        need: None,
    },
    Step {
        id: "editor.status",
        since: "3.0.0",
        spot: Some(Spot::EditorStatus),
        title: tx("Statusleiste, Gehe zu Zeile", "Status bar, go to line"),
        body: tx(
            "Die Statusleiste zeigt Zeile und Spalte, Einrückung, Kodierung und Zeilenende – beide werden beim Öffnen erkannt und lassen sich per Klick umstellen (UTF-8, UTF-16, Windows-1252; LF oder CRLF). Strg+G springt zu einer Zeile.",
            "The status bar shows line and column, indentation, encoding and line endings – both are detected on opening and can be switched with a click (UTF-8, UTF-16, Windows-1252; LF or CRLF). Ctrl+G jumps to a line.",
        ),
        prep: EDITOR_FILE,
        done: None,
        need: None,
    },
    Step {
        id: "editor.minimap",
        since: "3.0.0",
        spot: Some(Spot::EditorMinimap),
        title: tx("Minimap", "Minimap"),
        body: tx(
            "Die Minimap zeigt die ganze Datei im Kleinen; Klicken oder Ziehen rollt dorthin. Große Dateien (ab 20 MB) öffnen ohne Minimap und Umbruch und bleiben auch mit mehreren hundert MB flüssig.",
            "The minimap shows the whole file in small; click or drag it to scroll there. Large files (from 20 MB) open without minimap and wrapping and stay smooth even with several hundred MB.",
        ),
        prep: EDITOR_FILE,
        done: None,
        need: None,
    },
    Step {
        id: "editor.disk",
        since: "3.0.0",
        spot: None,
        title: tx("Auf der Platte geändert", "Changed on disk"),
        body: tx(
            "Ändert ein anderes Programm (z. B. Claude Code) eine offene Datei, lädt der Editor sie still neu. Hast du dort ungespeicherte Änderungen, fragt eine Leiste: „Meine behalten“, „Von der Platte laden“ oder „Unterschied anzeigen“. Gespeichert wird immer über eine temporäre Datei, damit nichts halb geschrieben zurückbleibt.",
            "When another program (e.g. Claude Code) changes an open file, the editor reloads it quietly. If you have unsaved changes there, a bar asks: “Keep mine”, “Load from disk” or “Show difference”. Saving always goes through a temporary file, so nothing is left half written.",
        ),
        prep: EDITOR_FILE,
        done: None,
        need: None,
    },
    Step {
        id: "editor.search",
        since: "3.0.0",
        spot: Some(Spot::EditorSearchFiles),
        title: tx("In Dateien suchen", "Search in files"),
        body: tx(
            "Strg+Umschalt+F durchsucht den geöffneten Ordner – auch mit regulären Ausdrücken; .gitignore wird beachtet, Binärdateien und große Dateien werden übersprungen. Ein Klick auf einen Treffer öffnet die Datei an der Stelle, F4 und Umschalt+F4 springen zum nächsten und vorigen.",
            "Ctrl+Shift+F searches the opened folder – also with regular expressions; .gitignore is respected, binary and large files are skipped. A click on a result opens the file at that spot, F4 and Shift+F4 jump to the next and previous one.",
        ),
        prep: &[Prep::NoSettings, Prep::Palette(false), Prep::NoTools, Prep::Editor, Prep::EditorFile, Prep::EditorFind(false), Prep::EditorSearch(true)],
        done: None,
        need: None,
    },
    Step {
        id: "editor.settings",
        since: "3.0.0",
        spot: Some(Spot::TabEditor),
        title: tx("Einstellungen → Editor", "Settings → Editor"),
        body: tx(
            "Hier stellst du die Schrift des Editors ein (Standard: die Terminalschrift), ab welcher Größe eine Datei als groß gilt und ob automatisch gespeichert wird – nur Dateien, die es schon gibt.",
            "Here you choose the editor font (default: the terminal font), from which size a file counts as large and whether files are saved automatically – only files that already exist.",
        ),
        prep: &[Prep::Palette(false), Prep::NoTools, Prep::EditorSearch(false), Prep::Editor, Prep::Settings(S_EDITOR)],
        done: None,
        need: None,
    },
    Step {
        id: "editor.search_settings",
        since: "3.0.0",
        spot: Some(Spot::EditorSearchSettings),
        title: tx("Suche in Dateien einstellen", "Search in files settings"),
        body: tx(
            "„Ignorierte einbeziehen“ ist die Vorgabe für den Schalter im Suchfenster; Ausschlussmuster (wie in .gitignore, z. B. node_modules oder *.min.js) gelten immer.",
            "“Include ignored” is the default of the switch in the search window; exclude patterns (as in .gitignore, e.g. node_modules or *.min.js) always apply.",
        ),
        prep: &[Prep::Editor, Prep::Settings(S_EDITOR)],
        done: None,
        need: None,
    },
    Step {
        id: "editor.agents",
        since: "3.0.0",
        spot: Some(Spot::EditorAgentSwitches),
        title: tx("Agenten und der Editor", "Agents and the editor"),
        body: tx(
            "Agenten dürfen offene Dateien ab der Stufe „Nur lesen“ lesen. Öffnen, Bearbeiten und Speichern brauchen die Stufe „Steuern“ und je einen eigenen Schalter hier – alle standardmäßig aus. Änderungen eines Agenten bleiben ungespeichert und lassen sich rückgängig machen; die Tastatur nimmt ein Agent dir nie.",
            "Agents may read open files from level “Read only”. Opening, editing and saving need level “Control” plus their own switch here – all off by default. Changes by an agent stay unsaved and can be undone; an agent never takes your keyboard focus.",
        ),
        prep: &[Prep::Editor, Prep::Settings(S_EDITOR)],
        done: None,
        need: None,
    },
];

/// Kapitel „Todo-Liste & Expertenmodus“.
const TODO: &[Step] = &[
    Step {
        id: "todo.list",
        since: "2.0.0",
        spot: Some(Spot::TodoInput),
        title: tx("Die einfache Liste", "The simple list"),
        body: tx(
            "Ohne Expertenmodus ist die Todo-Liste eine einfache Liste: schreiben, Enter, abhaken. Tags (#tag) und Fälligkeit gibt es über „Mehr …“. Leg ein To-do an, um weiterzumachen.",
            "Without expert mode the to-do list is a simple list: type, press Enter, tick off. Tags (#tag) and due dates come with “More …”. Add a to-do to continue.",
        ),
        prep: TODO_SIMPLE,
        done: Some(todos_grew),
        need: None,
    },
    Step {
        id: "todo.expert",
        since: "2.0.0",
        spot: Some(Spot::ExpertToggle),
        title: tx("Expertenmodus einschalten", "Switch expert mode on"),
        body: tx(
            "Schalte den Expertenmodus ein. Deine einfache Liste bleibt, wie sie ist, und ist wieder da, sobald du ihn ausschaltest.",
            "Switch expert mode on. Your simple list stays as it is and is back as soon as you switch it off.",
        ),
        prep: TODO_SIMPLE,
        done: Some(expert_on),
        need: None,
    },
    Step {
        id: "todo.project",
        since: "2.0.0",
        spot: Some(Spot::PlanProjects),
        title: tx("Projekte mit Schlüssel", "Projects with a key"),
        body: tx(
            "Im Expertenmodus hat jedes Projekt einen Schlüssel und eigene Typen, Felder, Status und Workflows. Die Tour hat dafür das Projekt „Tutorial“ angelegt; „+ Neues Projekt“ startet den Assistenten.",
            "In expert mode every project has a key and its own types, fields, statuses and workflows. The tour created the project “Tutorial” for this; “+ New project” starts the wizard.",
        ),
        prep: PLAN,
        done: None,
        need: None,
    },
    Step {
        id: "todo.ticket",
        since: "2.0.0",
        spot: Some(Spot::PlanNewTicket),
        title: tx("Ein Ticket anlegen", "Create a ticket"),
        body: tx(
            "Leg mit „Neues Ticket“ (Strg+N) ein Beispiel-Ticket an: Typ wählen, Titel eingeben, speichern. Es verschwindet mit dem Tutorial-Projekt.",
            "Create an example ticket with “New ticket” (Ctrl+N): choose a type, enter a title, save. It disappears together with the tutorial project.",
        ),
        prep: &[Prep::NoSettings, Prep::Tasks, Prep::Expert(true), Prep::PlanProject, Prep::PlanTab("tickets")],
        done: Some(tickets_grew),
        need: None,
    },
    Step {
        id: "todo.boards",
        since: "2.0.0",
        spot: Some(Spot::PlanTabBoards),
        title: tx("Boards", "Boards"),
        body: tx(
            "Boards zeigen Tickets in Spalten nach Status; eine Karte in eine andere Spalte ziehen ändert ihren Status. Öffne den Reiter „Boards“, um weiterzumachen.",
            "Boards show tickets in columns by status; dragging a card into another column changes its status. Open the “Boards” tab to continue.",
        ),
        prep: &[Prep::NoSettings, Prep::Tasks, Prep::Expert(true), Prep::PlanProject, Prep::PlanTab("tickets")],
        done: Some(boards_shown),
        need: None,
    },
    Step {
        id: "todo.sprints",
        since: "2.0.0",
        spot: Some(Spot::PlanTabBacklog),
        title: tx("Backlog und Sprints", "Backlog and sprints"),
        body: tx(
            "Mit Sprints (in den Projekteinstellungen an) plant der Backlog die Sprints: Tickets in einen Sprint ziehen, ihn starten und abschließen. Öffne den Backlog, um weiterzumachen.",
            "With sprints (switched on in the project settings) the backlog plans the sprints: drag tickets into a sprint, start it and complete it. Open the backlog to continue.",
        ),
        prep: PLAN,
        done: Some(backlog_shown),
        need: None,
    },
    Step {
        id: "todo.search",
        since: "2.0.0",
        spot: Some(Spot::PlanTabSearch),
        title: tx("Suchen mit Abfragen", "Search with queries"),
        body: tx(
            "Der Reiter „Suche“ findet Tickets mit einer Abfragesprache, z. B. type = Task AND status != Done. Öffne ihn, um weiterzumachen.",
            "The “Search” tab finds tickets with a query language, e.g. type = Task AND status != Done. Open it to continue.",
        ),
        prep: PLAN,
        done: Some(search_shown),
        need: None,
    },
    Step {
        id: "todo.query",
        since: "2.0.0",
        spot: Some(Spot::QueryBar),
        title: tx("Die Abfrage-Leiste", "The query bar"),
        body: tx(
            "Abfrage eintippen und Enter drücken; Vorschläge ergänzen Felder und Werte. Die Hilfe an der Leiste erklärt jedes Schlüsselwort mit Beispielen, gespeicherte Filter merken sich Abfragen.",
            "Type a query and press Enter; suggestions complete fields and values. The help at the bar explains every keyword with examples, saved filters remember queries.",
        ),
        prep: &[Prep::NoSettings, Prep::Tasks, Prep::Expert(true), Prep::PlanProject, Prep::PlanTab("search")],
        done: None,
        need: None,
    },
    Step {
        id: "todo.end",
        since: "2.0.0",
        spot: Some(Spot::ExpertToggle),
        title: tx("Zum Schluss", "Wrapping up"),
        body: tx(
            "Hier schaltest du jederzeit zwischen einfacher Liste und Expertenmodus um. Am Ende des Kapitels stellt die Tour den Expertenmodus wie vorher ein und entfernt das Tutorial-Projekt mit seinen Tickets – es sei denn, du behältst es.",
            "This switches between the simple list and expert mode at any time. At the end of the chapter the tour sets expert mode back to how it was and removes the tutorial project with its tickets – unless you keep them.",
        ),
        prep: &[Prep::NoSettings, Prep::Tasks],
        done: None,
        need: None,
    },
];

/// Kapitel „Claude Code im Detail“.
const CLAUDE: &[Step] = &[
    Step {
        id: "claude.mode",
        since: "1.0.0",
        spot: Some(Spot::ClaudeMode),
        title: tx("Claude-Modus", "Claude mode"),
        body: tx(
            "Der Claude-Modus zeigt alles rund um Claude Code: den Funken-Knopf, Status, Kosten und Verlauf. Ohne Claude Code kannst du ihn hier abschalten.",
            "Claude mode shows everything around Claude Code: the spark button, status, costs and history. Without Claude Code you can switch it off here.",
        ),
        prep: &[Prep::Terminals, Prep::Palette(false), Prep::NoTools, Prep::Settings(S_CLAUDE)],
        done: None,
        need: Some(Need::Claude),
    },
    Step {
        id: "claude.defaults",
        since: "2.3.0",
        spot: Some(Spot::ClaudeDefaults),
        title: tx("Standards für neue Sitzungen", "Defaults for new sessions"),
        body: tx(
            "Modell, Aufwand, Berechtigungsmodus und mehr für neue Claude-Sitzungen werden hier vorgewählt – der Dialog „Neue Sitzung“ ändert sie je Sitzung.",
            "Model, effort, permission mode and more for new Claude sessions are preselected here – the “New session” dialog changes them per session.",
        ),
        prep: &[Prep::Terminals, Prep::Settings(S_CLAUDE)],
        done: None,
        need: None,
    },
    Step {
        id: "claude.budget",
        since: "1.0.0",
        spot: Some(Spot::ClaudeBudget),
        title: tx("Budget", "Budget"),
        body: tx(
            "Ein Tages- und ein Sitzungsbudget melden sich, sobald die geschätzten Kosten den Betrag überschreiten. Die Preise für die Schätzung lassen sich darunter anpassen.",
            "A daily and a per-session budget notify you once the estimated costs exceed the amount. The prices for the estimate can be adjusted below.",
        ),
        prep: &[Prep::Terminals, Prep::Settings(S_CLAUDE)],
        done: None,
        need: None,
    },
    Step {
        id: "claude.start",
        since: "1.0.0",
        spot: Some(Spot::SidebarClaude),
        title: tx("Eine Claude-Sitzung starten", "Start a Claude session"),
        body: tx(
            "Der Funken-Knopf öffnet „Neue Claude-Sitzung“: Ordner, Modell, Aufwand, Berechtigungsmodus und mehr. Die Tour startet Claude nicht für dich.",
            "The spark button opens “New Claude session”: folder, model, effort, permission mode and more. The tour does not start Claude for you.",
        ),
        prep: HUB,
        done: None,
        need: Some(Need::Claude),
    },
    Step {
        id: "claude.status",
        since: "1.0.0",
        spot: Some(Spot::SessionRow),
        title: tx("Status auf einen Blick", "Status at a glance"),
        body: tx(
            "Jede Claude-Sitzung zeigt ihren Status: arbeitet, Freigabe?, bereit. Sitzungen, die dich brauchen, zählt der Fenstertitel; Benachrichtigungen stellst du unter Einstellungen → Benachrichtigungen ein.",
            "Each Claude session shows its status: working, approval?, ready. Sessions that need you are counted in the window title; notifications are set up under Settings → Notifications.",
        ),
        prep: HUB_SESSION,
        done: None,
        need: None,
    },
    Step {
        id: "claude.history",
        since: "1.0.0",
        spot: Some(Spot::SidebarHistory),
        title: tx("Verlauf", "History"),
        body: tx(
            "Die Uhr öffnet den Claude-Verlauf: frühere Konversationen finden und fortsetzen. Öffne ihn, um weiterzumachen.",
            "The clock opens the Claude history: find earlier conversations and resume them. Open it to continue.",
        ),
        prep: HUB,
        done: Some(history_opened),
        need: None,
    },
    Step {
        id: "claude.prompts",
        since: "1.0.0",
        spot: Some(Spot::SidebarPrompts),
        title: tx("Prompt-Bibliothek", "Prompt library"),
        body: tx(
            "Oft gebrauchte Prompts liegen in der Prompt-Bibliothek und gehen mit einem Klick an eine Sitzung. Öffne sie, um weiterzumachen.",
            "Keep prompts you use often in the prompt library and send them to a session with one click. Open it to continue.",
        ),
        prep: HUB,
        done: Some(prompts_opened),
        need: None,
    },
    Step {
        id: "claude.queue",
        since: "1.0.0",
        spot: Some(Spot::SidebarComposer),
        title: tx("Composer und Warteschlange", "Composer and queue"),
        body: tx(
            "Im Prompt-Composer ({key:composer}) schreibst du längere Prompts in Ruhe. Jede Claude-Sitzung hat außerdem eine Warteschlange (Rechtsklick → „Warteschlange …“), die Prompts nacheinander sendet, sobald Claude bereit ist. Öffne den Composer, um weiterzumachen.",
            "The prompt composer ({key:composer}) lets you write longer prompts in peace. Each Claude session also has a queue (right-click → “Queue …”) that sends prompts one after the other whenever Claude is ready. Open the composer to continue.",
        ),
        prep: HUB,
        done: Some(composer_opened),
        need: None,
    },
];

/// Kapitel „Chats mit lokalen Modellen“ (geht ohne echtes Modell: es werden nur die Stellen gezeigt).
const CHAT: &[Step] = &[
    Step {
        id: "chat.intro",
        since: "2.2.0",
        spot: Some(Spot::SidebarChat),
        title: tx("Lokale Modelle", "Local models"),
        body: tx(
            "Chats sprechen mit einem lokalen Modell – Ollama oder ein anderer OpenAI-kompatibler Server auf diesem Rechner oder im privaten Netz. Andere Adressen lässt Paneldeck nicht zu.",
            "Chats talk to a local model – Ollama or another OpenAI-compatible server on this computer or in your private network. Paneldeck does not allow other addresses.",
        ),
        prep: HUB,
        done: None,
        need: Some(Need::Ollama),
    },
    Step {
        id: "chat.models",
        since: "2.2.0",
        spot: Some(Spot::TabModels),
        title: tx("Einstellungen → Modelle", "Settings → Models"),
        body: tx(
            "Server und Modelle richtest du in diesem Reiter ein.",
            "Servers and models are set up in this tab.",
        ),
        prep: &[Prep::Terminals, Prep::Palette(false), Prep::NoTools, Prep::Settings(S_MODELS)],
        done: None,
        need: None,
    },
    Step {
        id: "chat.server",
        since: "2.2.0",
        spot: Some(Spot::ModelsServers),
        title: tx("Einen Server einrichten", "Set up a server"),
        body: tx(
            "Füge einen Server hinzu – für Ollama auf diesem Rechner reicht der Vorschlag http://localhost:11434 – und klicke auf „Verbinden“, um seine Modelle abzurufen. API-Schlüssel werden verschlüsselt gespeichert.",
            "Add a server – for Ollama on this computer the suggestion http://localhost:11434 is enough – and click “Connect” to fetch its models. API keys are stored encrypted.",
        ),
        prep: &[Prep::Terminals, Prep::Settings(S_MODELS)],
        done: None,
        need: None,
    },
    Step {
        id: "chat.levels",
        since: "2.2.0",
        spot: Some(Spot::ModelsServers),
        title: tx("Was ein Modell darf", "What a model may do"),
        body: tx(
            "Jedes Modell hat eine eigene Stufe: Aus (nur chatten), Nur lesen (sieht Sitzungen und To-dos) oder Steuern (darf tippen und ändern). Schreibende Werkzeuge fragen immer erst nach: Erlauben, Immer für diesen Chat oder Ablehnen.",
            "Each model has its own level: Off (just chatting), Read only (sees sessions and to-dos) or Control (may type and change things). Writing tools always ask first: Allow, Always for this chat or Deny.",
        ),
        prep: &[Prep::Terminals, Prep::Settings(S_MODELS)],
        done: None,
        need: None,
    },
    Step {
        id: "chat.commands",
        since: "3.1.0",
        spot: Some(Spot::ModelsCommands),
        title: tx("Befehle und Fragen", "Commands and questions"),
        body: tx(
            "Ein Modell mit der Stufe „Steuern“ kann lange Befehle im Hintergrund starten: Jeder läuft in einem eigenen Terminal unter dem Chat, am Ende kommt eine Nachricht in den Chat. Hier stellst du Zeitlimit und Anzahl ein. Außerdem kann ein Modell dir Fragen mit Auswahl stellen – mit „(Empfohlen)“ und „Andere …“.",
            "A model with level “Control” can start long commands in the background: each runs in its own terminal below the chat, and a message comes into the chat when it ends. Set the time limit and the number here. A model can also ask you questions with choices – with “(Recommended)” and “Other …”.",
        ),
        prep: &[Prep::Terminals, Prep::Settings(S_MODELS)],
        done: None,
        need: None,
    },
    Step {
        id: "chat.new",
        since: "2.2.0",
        spot: Some(Spot::SidebarChat),
        title: tx("Einen Chat starten", "Start a chat"),
        body: tx(
            "„Neuer Chat“ fragt nach Server und Modell; der Chat läuft dann als Sitzung im Tutorial-Projekt. Öffne den Dialog, um weiterzumachen.",
            "“New chat” asks for the server and the model; the chat then runs as a session in the tutorial project. Open the dialog to continue.",
        ),
        prep: HUB,
        done: Some(chat_started),
        need: None,
    },
    Step {
        // Der Schalter steht nur in einem offenen Chat (das Tutorial legt keinen an): Schritt ohne Ziel.
        id: "chat.plan",
        since: "3.1.0",
        spot: None,
        title: tx("Erst planen, dann ändern", "Plan first, then change"),
        body: tx(
            "Neben „Senden“ steht im Chat der Schalter „Plan“ (Umschalt+Tab) mit einer Stufe von „niedrig“ bis „maximal“. Ist er an, recherchiert das Modell zuerst nur lesend und schreibt einen Plan – Schreib-Werkzeuge sperrt Paneldeck so lange. Der Plan erscheint als Karte: „Ausführen“ gibt ihn frei und füllt die To-do-Liste, „Änderungen anfragen“ lässt ihn überarbeiten, „Verwerfen“ beendet den Plan-Modus.",
            "Next to “Send” a chat has the “Plan” toggle (Shift+Tab) with a level from “low” to “max”. When it is on, the model first researches read-only and writes a plan – Paneldeck locks the writing tools until then. The plan appears as a card: “Execute” approves it and fills the to-do list, “Request changes” has it revised, “Discard” ends the plan mode.",
        ),
        prep: HUB,
        done: None,
        need: None,
    },
];

/// Kapitel „Agenten-Schnittstelle (MCP)“.
const MCP: &[Step] = &[
    Step {
        id: "mcp.levels",
        since: "1.0.0",
        spot: Some(Spot::McpLevel),
        title: tx("Drei Stufen", "Three levels"),
        body: tx(
            "Aus: Agenten bekommen nichts. Nur lesen: Sie sehen Sitzungen, Terminalinhalt, Kosten und To-dos. Steuern: Sie dürfen auch tippen, Befehle ausführen und eigene Sitzungen anlegen und schließen. Terminalinhalt ist für sie nur Information, nie eine Anweisung.",
            "Off: agents get nothing. Read only: they see sessions, terminal content, costs and to-dos. Control: they may also type, run commands and create and close their own sessions. Terminal content is information for them, never an instruction.",
        ),
        prep: &[Prep::Terminals, Prep::Palette(false), Prep::NoTools, Prep::Settings(S_AGENTS)],
        done: None,
        need: None,
    },
    Step {
        id: "mcp.register",
        since: "1.0.0",
        spot: Some(Spot::McpRegister),
        title: tx("In Claude Code eintragen", "Register with Claude Code"),
        body: tx(
            "„Registrieren“ trägt Paneldeck in Claude Code ein (Benutzerebene) – oder kopiere den Befehl bzw. das JSON für andere Agenten. Wird eine portable .exe verschoben, neu registrieren.",
            "“Register” adds Paneldeck to Claude Code (user scope) – or copy the command or the JSON for other agents. If a portable .exe moves, register again.",
        ),
        prep: &[Prep::Terminals, Prep::Settings(S_AGENTS)],
        done: None,
        need: Some(Need::Claude),
    },
    Step {
        id: "mcp.system_prompt",
        since: "2.1.0",
        spot: Some(Spot::McpSysprompt),
        title: tx("Systemprompt", "System prompt"),
        body: tx(
            "Agenten bekommen eine Anleitung, wie sie Paneldeck benutzen. Hier passt du sie mit einer eigenen system-prompt.md an.",
            "Agents get instructions on how to use Paneldeck. Here you adapt them with your own system-prompt.md.",
        ),
        prep: &[Prep::Terminals, Prep::Settings(S_AGENTS)],
        done: None,
        need: None,
    },
    Step {
        id: "mcp.log",
        since: "1.0.0",
        spot: Some(Spot::McpLog),
        title: tx("Aufrufprotokoll", "Call log"),
        body: tx(
            "Jeder Aufruf eines Agenten steht hier und in mcp-log.jsonl. Solange die Schnittstelle an ist, zeigt die Fensterleiste ihre Stufe; bei Aufrufen leuchtet sie auf.",
            "Every agent call is listed here and in mcp-log.jsonl. While the interface is on, the title bar shows its level and lights up on calls.",
        ),
        prep: &[Prep::Terminals, Prep::Settings(S_AGENTS)],
        done: None,
        need: None,
    },
];

/// Kapitel „Profile, Kommandozeile & Datenordner“.
const DATA: &[Step] = &[
    Step {
        id: "data.profiles",
        since: "1.0.0",
        spot: Some(Spot::TabProfiles),
        title: tx("Shell-Profile", "Shell profiles"),
        body: tx(
            "Profile starten eine Shell mit eigenem Programm, eigenen Argumenten und Variablen oder als Administrator. Projekte und Ordner können ein Standardprofil haben.",
            "Profiles start a shell with its own program, arguments and variables, or as administrator. Projects and folders can have a default profile.",
        ),
        prep: &[Prep::Terminals, Prep::Palette(false), Prep::NoTools, Prep::Settings(S_PROFILES)],
        done: None,
        need: None,
    },
    Step {
        id: "data.folder",
        since: "2.1.0",
        spot: Some(Spot::AboutInfo),
        title: tx("Datenordner und Instanzen", "Data folder and instances"),
        body: tx(
            "Hier steht, wo Paneldeck seine Daten ablegt: portabel neben der .exe, installiert unter %APPDATA%\\Paneldeck. --data-dir <ordner> oder PANELDECK_DATA wählen einen anderen Ordner, --profile <name> startet eine eigene Instanz mit eigenen Daten; paneldeck --help listet alle Optionen.",
            "This shows where Paneldeck keeps its data: portable next to the .exe, installed under %APPDATA%\\Paneldeck. --data-dir <folder> or PANELDECK_DATA choose another folder, --profile <name> starts a separate instance with its own data; paneldeck --help lists all options.",
        ),
        prep: &[Prep::Terminals, Prep::Settings(S_ABOUT)],
        done: None,
        need: None,
    },
    Step {
        id: "data.backups",
        since: "2.0.0",
        spot: Some(Spot::DataBackups),
        title: tx("Sicherungen", "Backups"),
        body: tx(
            "Die Datenbank (To-dos, Tickets) wird automatisch gesichert; Intervall, Zahl der Kopien und Ordner stellst du hier ein. Aus der Liste darunter lässt sich eine Sicherung zurückspielen.",
            "The database (to-dos, tickets) is backed up automatically; interval, number of copies and folder are set here. A backup can be restored from the list below.",
        ),
        prep: &[Prep::Terminals, Prep::Settings(S_DATA)],
        done: None,
        need: None,
    },
    Step {
        id: "data.cleanup",
        since: "2.6.0",
        spot: Some(Spot::DataCleanup),
        title: tx("Aufräumen", "Clean up"),
        body: tx(
            "„Aufräumen …“ zeigt, was Platz belegt – Sicherungen, Protokolle, Verläufe – und löscht nur, was du anhakst.",
            "“Clean up …” shows what takes up space – backups, logs, histories – and deletes only what you tick.",
        ),
        prep: &[Prep::Terminals, Prep::Settings(S_DATA)],
        done: None,
        need: None,
    },
    Step {
        id: "data.updates",
        since: "2.1.0",
        spot: Some(Spot::AboutUpdates),
        title: tx("Updates", "Updates"),
        body: tx(
            "Paneldeck fragt GitHub nur nach Updates, wenn du es willst oder die automatische Prüfung einschaltest. Installierte und portable Fassung aktualisieren sich selbst; Downloads werden gegen SHA256SUMS geprüft.",
            "Paneldeck only asks GitHub for updates when you want it to or switch on the automatic check. Installed and portable copies update themselves; downloads are checked against SHA256SUMS.",
        ),
        prep: &[Prep::Terminals, Prep::Settings(S_ABOUT)],
        done: None,
        need: None,
    },
];

/// Kapitel „Weitere Werkzeuge“.
const TOOLS: &[Step] = &[
    Step {
        id: "tools.files",
        since: "1.0.0",
        spot: Some(Spot::SidebarFiles),
        title: tx("Dateibaum", "File tree"),
        body: tx(
            "Der Dateibaum ({key:files}) zeigt den Ordner der fokussierten Sitzung; eine Datei ins Terminal ziehen fügt ihren Pfad ein. Öffne oder schließe ihn, um weiterzumachen.",
            "The file tree ({key:files}) shows the folder of the focused session; drag a file into a terminal to insert its path. Open or close it to continue.",
        ),
        prep: HUB,
        done: Some(files_toggled),
        need: None,
    },
    Step {
        id: "tools.diff",
        since: "1.0.0",
        spot: Some(Spot::SidebarDiff),
        title: tx("Änderungen", "Changes"),
        body: tx(
            "In Git-Ordnern zeigt dieser Knopf die Änderungen ({key:diff}): Diff ansehen, stagen und committen, ohne Paneldeck zu verlassen.",
            "In Git folders this button shows the changes ({key:diff}): view the diff, stage and commit without leaving Paneldeck.",
        ),
        prep: HUB,
        done: None,
        need: None,
    },
    Step {
        id: "tools.templates",
        since: "2.4.0",
        spot: Some(Spot::TemplatesSection),
        title: tx("Vorlagen", "Templates"),
        body: tx(
            "Terminal-Vorlagen starten ein Terminal mit allen Einstellungen oder ein ganzes Layout – per Klick oder indem du sie in die Seitenleiste oder einen Bereich ziehst.",
            "Terminal templates start a terminal with all its settings or a whole layout – with a click or by dragging them into the sidebar or a pane.",
        ),
        prep: HUB,
        done: None,
        need: None,
    },
    Step {
        id: "tools.processes",
        since: "2.6.0",
        spot: Some(Spot::ProcsSection),
        title: tx("Prozesse", "Processes"),
        body: tx(
            "Lang laufende Programme wie Entwicklungs-Server leben hier: starten, stoppen und ihre Ausgabe ansehen, ohne ein eigenes Terminal.",
            "Long-running programs such as development servers live here: start, stop and watch their output without a terminal of their own.",
        ),
        prep: HUB,
        done: None,
        need: None,
    },
    Step {
        id: "tools.themes",
        since: "2.6.0",
        spot: Some(Spot::ThemeManage),
        title: tx("Themes", "Themes"),
        body: tx(
            "Wähle ein Theme oder bau dein eigenes: Der Theme-Editor ändert jede Farbe der Oberfläche und prüft den Kontrast.",
            "Choose a theme or build your own: the theme editor changes every colour of the interface and checks the contrast.",
        ),
        prep: &[Prep::Terminals, Prep::Palette(false), Prep::NoTools, Prep::Settings(S_APPEARANCE)],
        done: None,
        need: None,
    },
];

/// Alle Touren: zuerst die kurze Tour, dann die Kapitel.
pub(super) static TOURS: &[Tour] = &[
    Tour { id: "basics", title: tx("Kurze Tour", "Short tour"), summary: tx("Die wichtigsten Teile in etwa 3 Minuten", "The main parts in about 3 minutes"), steps: BASICS },
    Tour { id: "editor", title: tx("Der Editor", "The editor"), summary: tx("Dateien, Reiter, Teilungen, Suchen und Ersetzen, mehrere Cursor, Suche in Dateien, Agenten", "Files, tabs, splits, find and replace, multiple cursors, search in files, agents"), steps: EDITOR },
    Tour { id: "todo", title: tx("Todo-Liste & Expertenmodus", "To-do list & expert mode"), summary: tx("Liste, Projekte, Tickets, Boards, Sprints, Abfragen", "List, projects, tickets, boards, sprints, queries"), steps: TODO },
    Tour { id: "claude", title: tx("Claude Code im Detail", "Claude Code in detail"), summary: tx("Claude-Modus, Status, Kosten und Budget, Verlauf, Prompts, Warteschlange", "Claude mode, status, costs and budget, history, prompts, queue"), steps: CLAUDE },
    Tour { id: "chat", title: tx("Chats mit lokalen Modellen", "Chats with local models"), summary: tx("Ollama einrichten, Modelle verbinden, Stufen und Freigaben", "Set up Ollama, connect models, levels and approvals"), steps: CHAT },
    Tour { id: "mcp", title: tx("Agenten-Schnittstelle (MCP)", "Agent interface (MCP)"), summary: tx("Stufen, in Claude Code eintragen, Systemprompt", "Levels, register with Claude Code, system prompt"), steps: MCP },
    Tour { id: "data", title: tx("Profile, Kommandozeile & Datenordner", "Profiles, command line & data folder"), summary: tx("Profile, --profile, --data-dir, Sicherungen, Updates", "Profiles, --profile, --data-dir, backups, updates"), steps: DATA },
    Tour { id: "tools", title: tx("Weitere Werkzeuge", "More tools"), summary: tx("Dateibaum, Änderungen, Vorlagen, Prozesse, Themes", "File tree, changes, templates, processes, themes"), steps: TOOLS },
];

/// Version, mit der das Tutorial erscheint (Schritte dürfen bis zu ihr reichen, auch solange `Cargo.toml` noch davor steht).
#[cfg_attr(not(test), allow(dead_code))]
pub(super) const TUTORIAL_VERSION: &str = "3.0.0";

/// Kennung der Tour „Was ist neu“ (ihre Schritte stellt `whats_new_steps` zusammen).
pub(super) const WHATS_NEW: &str = "whatsnew";

/// Titel der Tour „Was ist neu“.
pub(super) const WHATS_NEW_TITLE: Txt = tx("Neu in dieser Version", "New in this version");

pub(super) fn tour(id: &str) -> Option<&'static Tour> {
    TOURS.iter().find(|t| t.id == id)
}

/// Versionsnummer `X.Y.Z` als Zahlen (Unlesbares = 0.0.0).
pub(super) fn ver(s: &str) -> (u32, u32, u32) {
    let mut it = s.trim().split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    (it.next().unwrap_or(0), it.next().unwrap_or(0), it.next().unwrap_or(0))
}

/// Schritte, die nach Version `after` bis einschließlich `upto` dazukamen (Tour „Was ist neu“); jeder Schritt nur einmal.
pub(super) fn whats_new_steps(after: &str, upto: &str) -> Vec<&'static Step> {
    let (a, u) = (ver(after), ver(upto));
    let mut out: Vec<&'static Step> = Vec::new();
    for t in TOURS {
        for s in t.steps {
            let v = ver(s.since);
            if v > a && v <= u && !out.iter().any(|o| o.id == s.id) {
                out.push(s);
            }
        }
    }
    out
}
