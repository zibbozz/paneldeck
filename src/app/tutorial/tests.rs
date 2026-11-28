//! Tests des Tutorials (#143).
//!
//! **Ziele:** Die ganze App lässt sich ohne Fenster nicht zeichnen (sie startet Shells, Hook-Server, Datenbank). Darum
//! zwei Ebenen: (1) Quelltext-Prüfung – jede `Spot`-Kennung muss in der Oberfläche (außerhalb der Schritt-Tabelle)
//! gemeldet werden, und jede gemeldete Kennung gehört zu einem Schritt; der Compiler stellt sicher, dass es keine
//! Tippfehler gibt (Enum statt Text). (2) Laufzeit – der Smoke-Test des Test-Builds (`tools/smoke-mcp.ps1 -TutorialExe`)
//! geht jeden Schritt jeder Tour in der echten Oberfläche durch und prüft, dass sein Ziel gefunden wird.
//! Dazu kommen Tests der Texte (beide Sprachen), Versionen, Platzhalter, Vorbereitungen, Erkennungen, der Blase und der
//! Karte (ohne Fenster gezeichnet und angeklickt).

use super::marks::{self, Spot};
use super::overlay::{self, Bubble, CardClick, Click, EndClick};
use super::steps::{self, Prep, Probe, Step, Txt, TOURS};
use super::store;
use crate::state::Lang;
use eframe::egui::{self, Event, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};

/// Alle Quelltexte unter `src/` außer den Dateien, die Kennungen nur aufzählen (Tabelle, Definition, Tests, Test-Schnittstelle).
fn ui_sources() -> String {
    fn walk(dir: &std::path::Path, out: &mut String) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                walk(&p, out);
                continue;
            }
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            let in_tut = p.parent().and_then(|d| d.file_name()).is_some_and(|d| d == "tutorial");
            if !name.ends_with(".rs") || (in_tut && ["steps.rs", "marks.rs", "tests.rs", "hook.rs"].contains(&name)) {
                continue;
            }
            if let Ok(s) = std::fs::read_to_string(&p) {
                out.push_str(&s);
                out.push('\n');
            }
        }
    }
    let mut s = String::new();
    walk(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut s);
    s
}

/// Kommt `Spot::<name>` als ganzes Wort vor?
fn mentions(src: &str, name: &str) -> bool {
    let pat = format!("Spot::{name}");
    src.match_indices(&pat).any(|(i, _)| !src[i + pat.len()..].chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_'))
}

fn all_steps() -> impl Iterator<Item = (&'static steps::Tour, &'static Step)> {
    TOURS.iter().flat_map(|t| t.steps.iter().map(move |s| (t, s)))
}

#[test]
fn every_spot_is_marked_in_the_ui() {
    let src = ui_sources();
    assert!(src.len() > 100_000, "Quelltexte gefunden");
    let missing: Vec<&str> = Spot::ALL.iter().filter(|s| !mentions(&src, s.name())).map(|s| s.name()).collect();
    assert!(missing.is_empty(), "diese Ziele meldet keine Stelle der Oberfläche: {missing:?}");
}

#[test]
fn every_step_target_exists_and_every_spot_is_used() {
    for s in Spot::ALL {
        assert!(all_steps().any(|(_, st)| st.spot == Some(*s)), "Ziel {} gehört zu keinem Schritt", s.name());
    }
    let mut ids: Vec<&str> = Spot::ALL.iter().map(|s| s.id()).collect();
    ids.sort();
    let n = ids.len();
    ids.dedup();
    assert_eq!(ids.len(), n, "Kennungen der Ziele sind eindeutig");
}

fn check_txt(what: &str, x: Txt, long: bool) {
    for (lang, s) in [("de", x.de), ("en", x.en)] {
        assert!(!s.trim().is_empty(), "{what} ({lang}) leer");
        assert_eq!(s.trim(), s, "{what} ({lang}): Leerraum am Rand");
        assert!(!s.contains("TODO") && !s.contains("XXX"), "{what} ({lang}): Platzhalter");
    }
    assert!(!x.en.chars().any(|c| "äöüÄÖÜß„".contains(c)), "{what}: englischer Text mit deutschen Zeichen: {}", x.en);
    assert!(!x.de.contains('”'), "{what}: deutscher Text mit englischem Schlusszeichen: {}", x.de);
    if long {
        assert_ne!(x.de, x.en, "{what}: beide Sprachen gleich");
    }
}

#[test]
fn every_text_is_in_both_languages() {
    for t in TOURS {
        check_txt(&format!("Tour {}", t.id), t.title, false);
        check_txt(&format!("Tour {} (Zusammenfassung)", t.id), t.summary, true);
        assert!(!t.steps.is_empty(), "Tour {} ohne Schritte", t.id);
    }
    for (t, s) in all_steps() {
        check_txt(&format!("{} / {} (Titel)", t.id, s.id), s.title, false);
        check_txt(&format!("{} / {} (Text)", t.id, s.id), s.body, true);
    }
    check_txt("Was ist neu", steps::WHATS_NEW_TITLE, true);
    for n in [steps::Need::Claude, steps::Need::Ollama] {
        check_txt(&format!("Voraussetzung {n:?}"), n.note(), true);
        assert!(n.link().starts_with("https://"), "offizieller Link");
    }
}

#[test]
fn step_ids_are_unique_and_belong_to_their_tour() {
    let mut seen = std::collections::HashSet::new();
    for (t, s) in all_steps() {
        assert!(seen.insert(s.id), "Kennung doppelt: {}", s.id);
        assert!(s.id.starts_with(&format!("{}.", t.id)), "{} gehört zur Tour {}", s.id, t.id);
    }
    let mut tours: Vec<&str> = TOURS.iter().map(|t| t.id).collect();
    tours.dedup();
    assert_eq!(tours.len(), TOURS.len());
    assert!(!TOURS.iter().any(|t| t.id == steps::WHATS_NEW));
    assert_eq!(TOURS[0].id, "basics", "die kurze Tour steht vorn");
}

#[test]
fn every_step_carries_a_valid_version() {
    let cur = steps::ver(env!("CARGO_PKG_VERSION")).max(steps::ver(steps::TUTORIAL_VERSION));
    for (_, s) in all_steps() {
        let parts: Vec<&str> = s.since.split('.').collect();
        assert_eq!(parts.len(), 3, "{}: Version X.Y.Z", s.id);
        assert!(parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) && (p.len() == 1 || !p.starts_with('0'))), "{}: {}", s.id, s.since);
        let v = steps::ver(s.since);
        assert!(v >= (1, 0, 0) && v <= cur, "{}: {} liegt außerhalb", s.id, s.since);
    }
}

#[test]
fn key_placeholders_name_known_shortcuts() {
    for (_, s) in all_steps() {
        for text in [s.body.de, s.body.en] {
            let mut rest = text;
            while let Some(i) = rest.find("{key:") {
                let after = &rest[i + 5..];
                let j = after.find('}').unwrap_or_else(|| panic!("{}: offener Platzhalter", s.id));
                let id = &after[..j];
                assert!(crate::keys::DEFAULTS.iter().any(|(k, _)| *k == id), "{}: unbekanntes Kürzel {id}", s.id);
                rest = &after[j + 1..];
            }
            let filled = super::fill_keys(text, Lang::En, |k| format!("<{k}>"));
            assert!(!filled.contains("{key:"), "{}: nicht ersetzt", s.id);
        }
    }
    assert_eq!(super::fill_keys("a {key:new} b", Lang::En, |_| "Ctrl+T".into()), "a Ctrl+T b");
    assert_eq!(super::fill_keys("a {key:close_all} b", Lang::De, |_| String::new()), "a ohne Kürzel b");
    assert_eq!(super::fill_keys("kaputt {key:new", Lang::En, |_| "X".into()), "kaputt {key:new");
}

/// Wo ein Ziel liegt: Einstellungsreiter (Index) bzw. Bereich der Anwendung.
#[derive(Debug, PartialEq)]
enum Home {
    Settings(usize),
    Hub,
    Tasks,
    Plan,
    Palette,
    /// Editor-Reiter; `true` = braucht eine offene Datei (die Beispieldatei der Tour).
    Editor(bool),
    /// Reiter „Zeit“ mit diesem Unterreiter (#164).
    Time(super::super::time_ui::Sub),
}

fn home(s: Spot) -> Home {
    use Spot::*;
    match s {
        TabEditor | EditorSearchSettings | EditorAgentSwitches | EditorSaveActions => Home::Settings(super::super::editor_services::SETTINGS_TAB),
        EditorTree => Home::Editor(false),
        EditorTabBar | EditorSplit | EditorText | EditorFind | EditorStatus | EditorMinimap | EditorSearchFiles | EditorMenuBar => Home::Editor(true),
        TitlebarEditor | TitlebarTime => Home::Hub,
        TimeSubTabs | TimeNewAlarm | TimeOptions => Home::Time(super::super::time_ui::Sub::Alarms),
        TimePresets => Home::Time(super::super::time_ui::Sub::Timers),
        CalendarViews => Home::Time(super::super::time_ui::Sub::Calendar),
        ThemeManage | ReduceTransparency => Home::Settings(1),
        ClaudeMode | ClaudeDefaults | ClaudeBudget => Home::Settings(2),
        TabShortcuts => Home::Settings(5),
        TabProfiles => Home::Settings(6),
        McpLevel | McpRegister | McpSysprompt | McpSessionEnv | McpLog | AgentAccess => Home::Settings(7),
        DataBackups | DataCleanup => Home::Settings(8),
        AboutInfo | AboutUpdates | AboutTutorial => Home::Settings(9),
        TabModels | ModelsServers | ModelsCommands => Home::Settings(super::super::models_ui::SETTINGS_TAB),
        TodoInput | TodoList | ExpertToggle => Home::Tasks,
        PlanProjects | PlanNewTicket | PlanTabBoards | PlanTabBacklog | PlanTabSearch | QueryBar => Home::Plan,
        Palette => Home::Palette,
        TitlebarTabs | TitlebarTodo | Sidebar | SidebarNew | SidebarClaude | SidebarChat | SidebarComposer | SidebarPalette | SidebarPrompts | SidebarFiles | SidebarDiff
        | SidebarHistory | SidebarFooter | ProcsSection | TemplatesSection | ProjectBar | SessionRow | PaneSplit | PaneMinimize => Home::Hub,
    }
}

/// „Weiter“ führt immer zu einem sichtbaren Ziel: Die Vorbereitungen jedes Schritts öffnen den Bereich seines Ziels.
#[test]
fn preparations_open_the_home_of_each_target() {
    for (_, s) in all_steps() {
        let Some(spot) = s.spot else {
            continue;
        };
        let has = |p: Prep| s.prep.contains(&p);
        match home(spot) {
            Home::Settings(tab) => assert!(has(Prep::Settings(tab)), "{}: Einstellungen Reiter {tab} öffnen", s.id),
            Home::Hub => {
                assert!(has(Prep::Terminals) && has(Prep::NoSettings), "{}: Terminal Hub ohne Einstellungen", s.id);
                assert!(has(Prep::TutProject) || has(Prep::Session), "{}: im Tutorial-Projekt", s.id);
            }
            Home::Tasks => assert!(has(Prep::Tasks) && has(Prep::NoSettings), "{}: Todo-Liste", s.id),
            Home::Plan => assert!(has(Prep::Tasks) && has(Prep::Expert(true)) && has(Prep::PlanProject), "{}: Experten-Projekt", s.id),
            Home::Palette => assert!(has(Prep::Palette(true)), "{}: Palette offen", s.id),
            Home::Editor(file) => {
                assert!(has(Prep::Editor) && has(Prep::NoSettings), "{}: Editor-Reiter ohne Einstellungen", s.id);
                assert!(!file || has(Prep::EditorFile), "{}: Beispieldatei der Tour offen", s.id);
            }
            Home::Time(sub) => assert!(has(Prep::Time(sub)) && has(Prep::NoSettings), "{}: Reiter Zeit, Unterreiter {sub:?}", s.id),
        }
        // Ziele, die eine Sitzung brauchen
        if matches!(spot, Spot::SessionRow | Spot::PaneSplit | Spot::PaneMinimize) {
            assert!(has(Prep::Session), "{}: braucht eine Sitzung", s.id);
        }
        // Ziele des Editors, die erst eine Leiste oder ein Fenster zeigen
        match spot {
            Spot::EditorTree => assert!(has(Prep::EditorTree), "{}: Ordnerbaum einblenden", s.id),
            Spot::EditorFind => assert!(has(Prep::EditorFind(true)), "{}: Suchleiste öffnen", s.id),
            Spot::EditorSearchFiles => assert!(has(Prep::EditorSearch(true)), "{}: Suchfenster öffnen", s.id),
            _ => {}
        }
        // Erkennung „Suche geöffnet“ braucht eine geschlossene Suchleiste beim Betreten
        if s.id == "editor.find" {
            assert!(has(Prep::EditorFind(false)), "{}: Suchleiste zu Beginn zu", s.id);
        }
        for p in s.prep {
            if let Prep::Settings(i) = p {
                assert!(*i <= super::super::editor_services::SETTINGS_TAB, "{}: Reiter {i}", s.id);
            }
        }
    }
}

#[test]
fn plan_tab_keys_round_trip() {
    let mut keys: Vec<&str> = vec!["boards", "backlog", "search"];
    for (_, s) in all_steps() {
        for p in s.prep {
            if let Prep::PlanTab(k) = p {
                keys.push(k);
            }
        }
    }
    let mut plan = super::super::planning_ui::PlanUi::default();
    for k in keys {
        plan.set_tab_key(k);
        assert_eq!(plan.tab_key(), k, "Reiter {k}");
    }
    plan.set_tab_key("gibt-es-nicht");
    assert_eq!(plan.tab_key(), "search", "unbekannte Kennung ändert nichts");
}

fn step(id: &str) -> &'static Step {
    all_steps().find(|(_, s)| s.id == id).map(|(_, s)| s).unwrap_or_else(|| panic!("Schritt {id}"))
}

/// #393: Der Schritt zur Stufe eines Modells erklärt die Werkzeugliste mit genau dem Kennzeichen, das die Liste in
/// *Einstellungen → Modelle* neben standardmäßig ausgeschalteten Werkzeugen zeigt.
#[test]
fn the_model_level_step_names_the_off_by_default_mark() {
    let s = step("chat.levels");
    assert_eq!(s.spot, Some(Spot::ModelsServers));
    for (lang, text) in [(Lang::De, s.body.de), (Lang::En, s.body.en)] {
        let mark = crate::app::models_ui::default_off_mark(lang);
        assert!(text.contains(mark), "{lang:?}: {text}");
    }
}

#[test]
fn actions_are_recognised_from_the_state() {
    let base = Probe::default();
    let done = |id: &str, a: &Probe, b: &Probe| (step(id).done.expect("Erkennung"))(a, b);
    assert!(done("basics.new_terminal", &base, &Probe { sessions: 1, ..base.clone() }));
    assert!(!done("basics.new_terminal", &Probe { sessions: 1, ..base.clone() }, &base), "Schließen zählt nicht");
    let one = Probe { sessions: 1, names: 5, ..base.clone() };
    assert!(done("basics.rename", &one, &Probe { names: 6, ..one.clone() }));
    assert!(!done("basics.rename", &one, &one));
    assert!(done("basics.split", &Probe { panes: 1, ..base.clone() }, &Probe { panes: 2, ..base.clone() }));
    assert!(done("basics.minimize", &base, &Probe { minimized: 1, ..base.clone() }));
    assert!(done("basics.palette", &base, &Probe { palette: true, ..base.clone() }));
    assert!(!done("basics.palette", &Probe { palette: true, ..base.clone() }, &Probe { palette: true, ..base.clone() }), "schon offen beim Betreten");
    assert!(done("basics.todo", &base, &Probe { tasks: true, ..base.clone() }));
    assert!(done("basics.todo_add", &base, &Probe { todos: 1, ..base.clone() }));
    assert!(done("basics.todo_check", &base, &Probe { todos_done: 1, ..base.clone() }));
    assert!(done("basics.expert", &base, &Probe { expert: true, ..base.clone() }));
    assert!(done("basics.expert", &Probe { expert: true, ..base.clone() }, &base), "auch Ausschalten zählt");
    assert!(done("todo.expert", &base, &Probe { expert: true, ..base.clone() }));
    assert!(!done("todo.expert", &Probe { expert: true, ..base.clone() }, &base));
    assert!(done("todo.ticket", &base, &Probe { tickets: 1, ..base.clone() }));
    assert!(done("todo.boards", &Probe { plan_tab: "tickets", ..base.clone() }, &Probe { plan_tab: "boards", ..base.clone() }));
    assert!(done("todo.search", &Probe { plan_tab: "tickets", ..base.clone() }, &Probe { plan_tab: "search", ..base.clone() }));
    assert!(done("todo.sprints", &base, &Probe { plan_tab: "backlog", ..base.clone() }));
    assert!(done("claude.history", &base, &Probe { history: true, ..base.clone() }));
    assert!(done("claude.prompts", &base, &Probe { prompts: true, ..base.clone() }));
    assert!(done("claude.queue", &base, &Probe { composer: true, ..base.clone() }));
    assert!(done("chat.new", &base, &Probe { new_chat: true, ..base.clone() }));
    assert!(done("tools.files", &base, &Probe { files: true, ..base.clone() }));
    // Editor (#54)
    assert!(done("basics.editor", &base, &Probe { editor: true, ..base.clone() }));
    assert!(done("editor.intro", &base, &Probe { editor: true, ..base.clone() }));
    assert!(!done("editor.intro", &Probe { editor: true, ..base.clone() }, &Probe { editor: true, ..base.clone() }), "schon offen beim Betreten");
    assert!(done("editor.open", &Probe { editor_docs: 1, ..base.clone() }, &Probe { editor_docs: 2, ..base.clone() }));
    assert!(!done("editor.open", &Probe { editor_docs: 2, ..base.clone() }, &Probe { editor_docs: 1, ..base.clone() }), "Schließen zählt nicht");
    assert!(done("editor.split", &Probe { editor_groups: 1, ..base.clone() }, &Probe { editor_groups: 2, ..base.clone() }));
    assert!(done("editor.find", &base, &Probe { editor_find: true, ..base.clone() }));
    let one = Probe { editor_cursors: 1, ..base.clone() };
    assert!(done("editor.cursors", &one, &Probe { editor_cursors: 2, ..base.clone() }));
    assert!(!done("editor.cursors", &base, &one), "ein Cursor ist kein weiterer");
    assert!(step("editor.disk").done.is_none() && step("editor.disk").spot.is_none(), "Blase in der Mitte, nur Weiter");
    // Reiter „Zeit“ (#164)
    assert!(done("time.intro", &base, &Probe { time: true, ..base.clone() }));
    assert!(!done("time.intro", &Probe { time: true, ..base.clone() }, &Probe { time: true, ..base.clone() }), "schon offen beim Betreten");
    assert!(done("time.alarm", &base, &Probe { alarms: 1, ..base.clone() }));
    assert!(!done("time.alarm", &Probe { alarms: 2, ..base.clone() }, &Probe { alarms: 1, ..base.clone() }), "Löschen zählt nicht");
    // Ohne Erkennung: nur „Weiter“.
    assert!(step("basics.welcome").done.is_none() && step("mcp.register").done.is_none());
}

#[test]
fn whats_new_lists_the_steps_since_a_version() {
    let ids = |a: &str, b: &str| steps::whats_new_steps(a, b).iter().map(|s| s.id).collect::<Vec<_>>();
    let new = ids("2.5.0", "2.6.0");
    assert!(new.contains(&"tools.processes") && new.contains(&"tools.themes") && new.contains(&"data.cleanup"), "{new:?}");
    assert!(!new.contains(&"basics.split"), "ältere Funktionen nicht");
    assert!(ids("2.6.0", "2.6.0").is_empty(), "ohne neue Version nichts");
    assert!(ids("3.0.0", "2.6.0").is_empty(), "rückwärts nichts");
    let all = steps::whats_new_steps("0.0.0", "9.9.9").len();
    assert_eq!(all, all_steps().count(), "jeder Schritt einmal");
    assert!(ids("2.6.0", steps::TUTORIAL_VERSION).contains(&"basics.help"), "die Hilfe zum Tutorial ist neu in 3.0.0");
    // Der Editor (#54) kam mit 3.0.0: die Tour „Was ist neu“ für 2.x zeigt ihn, mit dem Schritt der kurzen Tour zuerst.
    let editor = ids("2.7.0", "3.0.0");
    assert!(editor.contains(&"basics.editor") && editor.contains(&"editor.agents") && editor.contains(&"editor.search"), "{editor:?}");
    // Ab 3.2.0 kommen Schritte dazu (Menüleiste und Formatieren #381, Aktionen beim Speichern #383); die übrigen der
    // Tour sind von 3.0.0.
    assert!(steps::tour("editor").is_some_and(|t| t.steps.iter().all(|s| ["3.0.0", "3.2.0", "3.3.0"].contains(&s.since))));
    let fmt = ids("3.1.0", "3.2.0");
    assert!(fmt.contains(&"editor.format") && !fmt.contains(&"editor.status"), "{fmt:?}");
    // Waagerechte Bildlaufleiste (#389) kam mit 3.2.0, Ziel ist das Textfeld.
    assert!(fmt.contains(&"editor.hscroll") && !fmt.contains(&"editor.minimap"), "{fmt:?}");
    assert_eq!(steps::tour("editor").and_then(|t| t.steps.iter().find(|s| s.id == "editor.hscroll")).map(|s| s.spot), Some(Some(Spot::EditorText)));
    assert_eq!(steps::tour("editor").and_then(|t| t.steps.iter().find(|s| s.id == "editor.format")).map(|s| s.spot), Some(Some(Spot::EditorMenuBar)));
    assert!(steps::tour("time").is_some_and(|t| t.steps.iter().all(|s| s.since == "3.2.0")));
    // Vergleich und Änderungsmarken (#385) kamen mit 3.2.0.
    let diff = ids("3.1.0", "3.2.0");
    assert!(diff.contains(&"editor.diff") && diff.contains(&"editor.markers"), "{diff:?}");
    // Tippen im Vergleich (#391) kam mit 3.3.0.
    let typing = ids("3.2.0", "3.3.0");
    assert!(typing.contains(&"editor.diff_type") && !typing.contains(&"editor.diff"), "{typing:?}");
    // Menü Auswahl und Kommentar (#394) kamen mit 3.3.0, Ziel ist die Menüleiste.
    assert!(typing.contains(&"editor.selection_menu") && !typing.contains(&"editor.cursors"), "{typing:?}");
    // Smart-Modus (#367) kam mit 3.3.0, Ziel ist die Statusleiste.
    assert!(typing.contains(&"editor.smart"), "{typing:?}");
    assert_eq!(steps::tour("editor").and_then(|t| t.steps.iter().find(|s| s.id == "editor.selection_menu")).map(|s| s.spot), Some(Some(Spot::EditorMenuBar)));
    // Pfadarten des Ziels (#392) kamen mit 3.2.0, im Kapitel Werkzeuge hinter dem Dateibaum.
    assert!(fmt.contains(&"tools.paths") && !fmt.contains(&"tools.files"), "{fmt:?}");
    let tools = steps::tour("tools").map(|t| t.steps.iter().map(|s| s.id).collect::<Vec<_>>()).unwrap_or_default();
    let at = |id: &str| tools.iter().position(|s| *s == id);
    assert_eq!(at("tools.paths"), at("tools.files").map(|i| i + 1), "{tools:?}");
    let step = steps::tour("tools").and_then(|t| t.steps.iter().find(|s| s.id == "tools.paths")).expect("tools.paths");
    assert_eq!(step.spot, Some(Spot::SidebarFiles));
}

/// Die Schritte im Editor arbeiten in einer eigenen Beispieldatei der Tour im Temp-Ordner, nie in einer des Nutzers.
#[test]
fn the_editor_sample_belongs_to_the_tour() {
    let p = super::super::App::tut_sample_path();
    assert!(p.starts_with(std::env::temp_dir()), "{p:?}");
    assert!(p.to_string_lossy().contains("paneldeck-tutorial"));
    for lang in [Lang::De, Lang::En] {
        let t = super::sample_text(lang);
        assert!(t.lines().count() >= 40, "genug Zeilen für Minimap und Suche");
    }
}

#[test]
fn version_bookkeeping_offers_whats_new_only_after_an_update() {
    let mut p = store::Progress::default();
    store::note_version(&mut p, "2.6.0");
    assert_eq!(p.last_version, "2.6.0");
    assert!(p.whats_new_after.is_empty(), "erster Start: Karte statt „Was ist neu“");
    store::note_version(&mut p, "2.6.0");
    assert!(p.whats_new_after.is_empty(), "gleiche Version");
    let mut p = store::Progress { last_version: "2.5.0".into(), ..Default::default() };
    store::note_version(&mut p, "2.6.0");
    assert_eq!((p.last_version.as_str(), p.whats_new_after.as_str()), ("2.6.0", "2.5.0"));
    let mut p = store::Progress { last_version: "2.6.0".into(), ..Default::default() };
    store::note_version(&mut p, "2.6.1");
    assert!(p.whats_new_after.is_empty(), "Update ohne neue Schritte: kein Hinweis");
    let mut p = store::Progress { last_version: "2.7.0".into(), ..Default::default() };
    store::note_version(&mut p, "2.6.0");
    assert_eq!(p.last_version, "2.7.0", "ältere Version (Rückkehr) ändert nichts");
}

/// „Fortsetzen“ macht beim gemerkten Schritt weiter – auch bei einer Tour, die schon einmal ganz durchlief und in einer
/// neuen Runde mittendrin beendet wurde (Smoke: kurze Tour fertig, neu gestartet, bei `basics.new_terminal` beendet,
/// „Fortsetzen“ begann wieder bei `basics.welcome`, weil `finished` vom ersten Durchlauf stehen blieb).
#[test]
fn continue_resumes_a_new_round_of_a_finished_tour() {
    let mut p = store::Progress { last_tour: "basics".into(), ..Default::default() };
    assert_eq!(p.resumable(), None, "nichts gemerkt");
    // Erster Durchlauf bis zum Ende: Haken, kein Schritt.
    p.tours.insert("basics".into(), store::TourProgress { step: String::new(), finished: true, skipped: vec![] });
    assert_eq!(p.resumable(), None, "zu Ende gelaufen: von vorn");
    // Neue Runde, bei Schritt 3 beendet.
    p.tours.get_mut("basics").unwrap().step = "basics.new_terminal".into();
    assert_eq!(p.resumable(), Some("basics"));
    assert!(p.tours["basics"].finished, "der Haken des ersten Durchlaufs bleibt");
    // Eine Tour, die nie fertig wurde, ebenso.
    p.last_tour = "todo".into();
    p.tours.insert("todo".into(), store::TourProgress { step: "todo.ticket".into(), finished: false, skipped: vec![] });
    assert_eq!(p.resumable(), Some("todo"));
    // „Was ist neu“ ist keine Tour der Tabelle: die App fängt dann von vorn an (`tut_resumable` sucht in `TOURS`).
    assert!(steps::tour(steps::WHATS_NEW).is_none());
}

/// Smoke (#143): Die Sitzungen des Tutorial-Projekts werden mit `@(Tut-Sessions)` gezählt. Ohne `@(…)` kommt eine
/// einzelne Sitzung als PSCustomObject zurück, dessen `.Count` in Windows PowerShell 5.1 `$null` ist – die Prüfung
/// „Sitzungen im Projekt Tutorial“ schlug dann genau mit der einen Sitzung der Tour fehl (die App war richtig).
#[test]
fn smoke_counts_tutorial_sessions_as_an_array() {
    let script = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tools").join("smoke-mcp.ps1")).unwrap();
    let uses: Vec<&str> = script.lines().filter(|l| l.contains("Tut-Sessions") && !l.trim_start().starts_with("function ") && !l.trim_start().starts_with('#')).collect();
    assert!(uses.len() >= 2, "{uses:?}");
    for l in uses {
        assert!(l.contains("@(Tut-Sessions)"), "ohne @(…) zählt PowerShell 5.1 eine einzelne Sitzung als null: {l}");
    }
}

#[test]
fn progress_file_is_tolerant() {
    let p: store::Progress = serde_json::from_str("{}").unwrap();
    assert_eq!(p, store::Progress::default());
    let p: store::Progress = serde_json::from_str(r#"{"card":"never","future":1,"tours":{"basics":{"step":"basics.split","x":2}}}"#).unwrap();
    assert_eq!(p.card, store::Card::Never);
    assert_eq!(p.tours["basics"].step, "basics.split");
    let dir = std::env::temp_dir().join(format!("paneldeck-test-tutorial-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    assert_eq!(store::load(&dir), store::Progress::default(), "fehlende Datei");
    let mut p = store::Progress { project: Some(7), todos: vec![3, 4], ..Default::default() };
    p.tours.entry("todo".into()).or_default().finished = true;
    store::save(&dir, &p);
    assert_eq!(store::load(&dir), p, "gespeichert und gelesen");
    std::fs::write(dir.join(store::FILE), "kaputt").unwrap();
    assert_eq!(store::load(&dir), store::Progress::default(), "unlesbar: frischer Stand");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn prerequisites_are_looked_up_in_path_only() {
    let dir = std::env::temp_dir().join(format!("paneldeck-test-tutorial-path-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = std::env::join_paths([dir.clone()]).unwrap();
    assert!(!super::prereq::in_path(&path, &["claude.cmd"]));
    std::fs::write(dir.join("claude.cmd"), "@echo off").unwrap();
    assert!(super::prereq::in_path(&path, &["claude.exe", "claude.cmd"]));
    assert!(!super::prereq::in_path(std::ffi::OsStr::new(""), &["claude.cmd"]));
    assert!(!super::prereq::missing(steps::Need::Ollama, 1), "ein eingerichteter Server genügt");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------- Ziele und Zeichnen ohne Fenster

fn input(time: f64, events: Vec<Event>) -> RawInput {
    RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 800.0))), time: Some(time), events, ..Default::default() }
}

#[test]
fn spots_are_only_recorded_while_a_tour_runs() {
    let ctx = egui::Context::default();
    let target = Rect::from_min_size(Pos2::new(40.0, 40.0), Vec2::new(30.0, 20.0));
    let run = |on: bool| {
        marks::begin_frame(on, Some(Spot::SidebarNew));
        let _ = ctx.run(input(0.1, vec![]), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                marks::spot_rect(ui, Spot::SidebarNew, target);
                let r = ui.button("x");
                use super::TutMark;
                let _ = r.spot(Spot::SidebarChat);
            });
        });
        (marks::seen(Spot::SidebarNew), marks::seen(Spot::SidebarChat))
    };
    assert_eq!(run(false), (None, None), "ohne Tour wird nichts gemeldet");
    let (a, b) = run(true);
    assert_eq!(a.map(|s| s.rect), Some(target));
    assert!(b.is_some_and(|s| s.shown.is_positive()));
    marks::begin_frame(true, None);
    assert!(marks::seen(Spot::SidebarNew).is_none(), "jedes Bild beginnt leer");
    marks::begin_frame(false, None);
}

#[test]
fn bubble_sits_next_to_the_target_and_inside_the_window() {
    let scr = Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 800.0));
    let size = Vec2::new(390.0, 220.0);
    for t in [
        Rect::from_min_size(Pos2::new(5.0, 5.0), Vec2::new(28.0, 28.0)),
        Rect::from_min_size(Pos2::new(1160.0, 770.0), Vec2::new(30.0, 20.0)),
        Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(260.0, 800.0)),
        Rect::from_min_size(Pos2::new(400.0, 300.0), Vec2::new(400.0, 200.0)),
    ] {
        let p = place(scr, Some(t), size);
        let b = Rect::from_min_size(p, size);
        assert!(scr.contains_rect(b), "{t:?}: Blase im Fenster ({b:?})");
        assert!(!b.intersects(t), "{t:?}: Blase verdeckt das Ziel nicht ({b:?})");
    }
    let c = Rect::from_min_size(place(scr, None, size), size).center();
    assert!((c - scr.center()).length() < 1.0, "ohne Ziel mittig");
}

fn place(scr: Rect, t: Option<Rect>, size: Vec2) -> Pos2 {
    overlay::place(scr, t, size)
}

/// Blase zeichnen und Texte bzw. Klick-Ergebnis liefern.
struct Harness {
    ctx: egui::Context,
    time: f64,
}

impl Harness {
    fn new() -> Self {
        Harness { ctx: egui::Context::default(), time: 0.0 }
    }

    fn frame<R>(&mut self, events: Vec<Event>, mut f: impl FnMut(&egui::Context) -> R) -> (R, Vec<String>) {
        self.time += 0.1;
        let mut out = None;
        let o = self.ctx.run(input(self.time, events), |ctx| {
            egui::CentralPanel::default().show(ctx, |_| {});
            out = Some(f(ctx));
        });
        let texts = o.shapes.iter().filter_map(|s| if let egui::Shape::Text(t) = &s.shape { Some(t.galley.text().to_string()) } else { None }).collect();
        (out.expect("gezeichnet"), texts)
    }

    fn click_text<R>(&mut self, label: &str, mut f: impl FnMut(&egui::Context) -> R) -> Vec<R> {
        // Erst zeichnen, um die Lage des Texts zu finden, dann klicken.
        let mut results = Vec::new();
        let mut pos = None;
        for _ in 0..3 {
            self.time += 0.1;
            let mut r = None;
            let o = self.ctx.run(input(self.time, vec![]), |ctx| {
                egui::CentralPanel::default().show(ctx, |_| {});
                r = Some(f(ctx));
            });
            results.push(r.unwrap());
            pos = o.shapes.iter().find_map(|s| if let egui::Shape::Text(t) = &s.shape { (t.galley.text() == label).then(|| t.visual_bounding_rect().center()) } else { None });
        }
        let p = pos.unwrap_or_else(|| panic!("„{label}“ gezeichnet"));
        let btn = |pressed| Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        for ev in [vec![Event::PointerMoved(p)], vec![btn(true)], vec![btn(false)]] {
            self.time += 0.1;
            let mut r = None;
            let _ = self.ctx.run(input(self.time, ev), |ctx| {
                egui::CentralPanel::default().show(ctx, |_| {});
                r = Some(f(ctx));
            });
            results.push(r.unwrap());
        }
        results
    }
}

fn bubble_of(lang: Lang, index: usize, total: usize) -> Bubble<'static> {
    let s = step("basics.split");
    Bubble { lang, tour: lang.t(TOURS[0].title.de, TOURS[0].title.en), title: lang.t(s.title.de, s.title.en), body: lang.t(s.body.de, s.body.en), note: None, index, total, waits: true, done: false, missing: false }
}

#[test]
fn bubble_shows_step_counter_and_buttons_in_both_languages() {
    let target = Some(Rect::from_min_size(Pos2::new(300.0, 200.0), Vec2::new(40.0, 24.0)));
    for (lang, labels) in [(Lang::En, ["Step 3/20", "Back", "Next", "Skip"]), (Lang::De, ["Schritt 3/20", "Zurück", "Weiter", "Überspringen"])] {
        let mut h = Harness::new();
        let b = bubble_of(lang, 3, 20);
        let (_, texts) = h.frame(vec![], |ctx| overlay::bubble(ctx, target, &b));
        let (_, texts2) = h.frame(vec![], |ctx| overlay::bubble(ctx, target, &b));
        let all: Vec<String> = texts.into_iter().chain(texts2).collect();
        for l in labels {
            assert!(all.iter().any(|t| t == l), "{lang:?}: „{l}“ fehlt in {all:?}");
        }
        assert!(all.iter().any(|t| t == b.title));
        for (label, want) in [(labels[1], Click::Back), (labels[2], Click::Next), (labels[3], Click::Skip)] {
            let mut h = Harness::new();
            let r = h.click_text(label, |ctx| overlay::bubble(ctx, target, &b).0);
            assert!(r.contains(&Some(want)), "{lang:?}: Klick auf „{label}“");
        }
    }
    // Letzter Schritt: „Fertig“ statt „Weiter“, kein „Überspringen“; erster Schritt: kein „Zurück“.
    let mut h = Harness::new();
    let b = bubble_of(Lang::En, 20, 20);
    let (_, t1) = h.frame(vec![], |ctx| overlay::bubble(ctx, target, &b));
    let (_, t2) = h.frame(vec![], |ctx| overlay::bubble(ctx, target, &b));
    let all: Vec<String> = t1.into_iter().chain(t2).collect();
    assert!(all.iter().any(|t| t == "Finish") && !all.iter().any(|t| t == "Skip"));
    let mut h = Harness::new();
    let b = bubble_of(Lang::En, 1, 20);
    let (_, t1) = h.frame(vec![], |ctx| overlay::bubble(ctx, target, &b));
    let (_, t2) = h.frame(vec![], |ctx| overlay::bubble(ctx, target, &b));
    assert!(!t1.iter().chain(t2.iter()).any(|t| t == "Back"));
}

#[test]
fn bubble_explains_a_missing_prerequisite_with_the_official_link() {
    let n = steps::Need::Claude;
    for lang in [Lang::En, Lang::De] {
        let mut b = bubble_of(lang, 2, 5);
        let note = lang.t(n.note().de, n.note().en);
        b.note = Some((note, n.link()));
        let mut h = Harness::new();
        let (_, t1) = h.frame(vec![], |ctx| overlay::bubble(ctx, None, &b));
        let (_, t2) = h.frame(vec![], |ctx| overlay::bubble(ctx, None, &b));
        let all: Vec<String> = t1.into_iter().chain(t2).collect();
        assert!(all.iter().any(|t| t == n.link()), "{lang:?}: Link sichtbar");
        assert!(all.iter().any(|t| t == note), "{lang:?}: Erklärung sichtbar");
    }
}

#[test]
fn first_start_card_offers_start_later_never() {
    for (lang, [start, later, never]) in [(Lang::En, ["Start", "Later", "Never"]), (Lang::De, ["Starten", "Später", "Nie"])] {
        for (label, want) in [(start, CardClick::Start), (later, CardClick::Later), (never, CardClick::Never)] {
            let mut h = Harness::new();
            let r = h.click_text(label, |ctx| overlay::card(ctx, lang));
            assert!(r.contains(&Some(want)), "{lang:?}: „{label}“");
        }
    }
}

#[test]
fn end_question_deletes_by_default_and_keeps_on_escape() {
    for (lang, del, keep) in [(Lang::En, "Delete", "Keep"), (Lang::De, "Löschen", "Behalten")] {
        let mut h = Harness::new();
        let r = h.click_text(del, |ctx| overlay::end_ask(ctx, lang, true, "x"));
        assert!(r.contains(&Some(EndClick::Delete)), "{lang:?}: Löschen");
        let mut h = Harness::new();
        let r = h.click_text(keep, |ctx| overlay::end_ask(ctx, lang, false, "x"));
        assert!(r.contains(&Some(EndClick::Keep)), "{lang:?}: Behalten");
    }
    let key = |k: egui::Key| Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE };
    let mut h = Harness::new();
    h.frame(vec![], |ctx| overlay::end_ask(ctx, Lang::En, true, "x"));
    let (r, _) = h.frame(vec![key(egui::Key::Enter)], |ctx| overlay::end_ask(ctx, Lang::En, true, "x"));
    assert_eq!(r, Some(EndClick::Delete), "Enter = Standard Löschen");
    let mut h = Harness::new();
    h.frame(vec![], |ctx| overlay::end_ask(ctx, Lang::En, true, "x"));
    let (r, _) = h.frame(vec![key(egui::Key::Escape)], |ctx| overlay::end_ask(ctx, Lang::En, true, "x"));
    assert_eq!(r, Some(EndClick::Keep), "Esc löscht nichts");
}

#[test]
fn pulse_follows_the_animation_setting() {
    assert_eq!(overlay::pulse(1.234, false), 1.0, "ohne Animationen fest");
    let v: Vec<f32> = (0..20).map(|i| overlay::pulse(i as f64 * 0.1, true)).collect();
    assert!(v.iter().all(|x| (0.5..=1.0).contains(x)));
    assert!(v.iter().any(|x| *x < 0.8) && v.iter().any(|x| *x > 0.9), "pulsiert: {v:?}");
}
