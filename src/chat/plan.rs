//! Plan-Modus der Chats (#358): Das Modell recherchiert zuerst nur lesend und schreibt einen Plan; geändert wird erst,
//! wenn der Nutzer den Plan freigibt (*Ausführen*). Kennt kein egui (die Plan-Karte steht in `app/chat_plan_ui.rs`).
//!
//! * [`PlanMode`]: Schalter neben *Senden* (gilt für die nächste Nachricht, Umschalt+Tab) mit der Stufe
//!   [`PlanLevel`] und die Stufe der laufenden Plan-Runde.
//! * [`Plan`]: der aktuelle Plan des Chats (`ChatFile::plan` in `chats/<id>.json`, öffentliches Datenformat ab 3.1.0) –
//!   Markdown und Stand ([`PlanStatus`]). [`ChatState::current_plan_markdown`] liefert ihn (z. B. für die Komprimierung,
//!   die ihn wörtlich behalten muss).
//! * [`is_read_only`]: die **eine** Stelle, die Werkzeuge in lesend/schreibend einteilt. In einer Plan-Runde liefert
//!   jeder andere Aufruf den Fehler `plan_mode` ([`gate`]) – durchgesetzt in `ChatState::tool_step`, nicht nur im Prompt.
//! * [`instructions`]: feste Anweisungen je Stufe (Aufbau, Tiefe, Recherche-Budget, Rückfragen) für den Systemprompt.
//! * Nach der Freigabe: Speichern als Markdown im Datenordner ([`save`], `plans/`), Schritte als To-do-Liste ([`todos`]).

use super::backend::{ChatBackend, Repaint, TurnContext};
use super::session::ChatState;
use super::store::{ChatTodo, Role, TodoStatus, ToolCall, ToolStatus};
use crate::mcp::McpLevel;
use crate::state::{ChatMeta, Lang};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Detailgrad eines Plans (Auswahl neben dem Schalter *Plan*).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize, Hash)]
#[serde(rename_all = "lowercase")]
pub enum PlanLevel {
    Low,
    #[default]
    Medium,
    High,
    #[serde(rename = "xhigh")]
    XHigh,
    Max,
}

impl PlanLevel {
    pub const ALL: [PlanLevel; 5] = [PlanLevel::Low, PlanLevel::Medium, PlanLevel::High, PlanLevel::XHigh, PlanLevel::Max];

    /// Name wie in der Datei und im Prompt (`low` … `max`).
    pub fn as_str(self) -> &'static str {
        match self {
            PlanLevel::Low => "low",
            PlanLevel::Medium => "medium",
            PlanLevel::High => "high",
            PlanLevel::XHigh => "xhigh",
            PlanLevel::Max => "max",
        }
    }

    /// Name in der Oberfläche.
    pub fn label(self, lang: Lang) -> &'static str {
        match self {
            PlanLevel::Low => lang.t("niedrig", "low"),
            PlanLevel::Medium => lang.t("mittel", "medium"),
            PlanLevel::High => lang.t("hoch", "high"),
            PlanLevel::XHigh => lang.t("sehr hoch", "xhigh"),
            PlanLevel::Max => lang.t("maximal", "max"),
        }
    }

    /// Kurzbeschreibung für die Auswahl.
    pub fn hint(self, lang: Lang) -> &'static str {
        match self {
            PlanLevel::Low => lang.t("kurzer Plan, wenige Lesezugriffe", "short plan, a few reads"),
            PlanLevel::Medium => lang.t("Ziel, Befunde, Schritte, Risiken, Tests", "goal, findings, steps, risks, tests"),
            PlanLevel::High => lang.t("Datei für Datei, Randfälle; klärt offene Punkte vorher", "file by file, edge cases; clarifies open points first"),
            PlanLevel::XHigh => lang.t("wie hoch, dazu Alternativen und Kompatibilität", "like high, plus alternatives and compatibility"),
            PlanLevel::Max => lang.t("extrem ausführlich, Rückfragen Pflicht, Recherche ohne Grenze", "extremely detailed, questions first, research without a limit"),
        }
    }

    /// Recherche-Budget: so viele Werkzeugaufrufe darf die Plan-Runde machen (`None` = ohne Grenze, nur *max*).
    pub fn research_budget(self) -> Option<u32> {
        match self {
            PlanLevel::Low => Some(5),
            PlanLevel::Medium => Some(12),
            PlanLevel::High => Some(25),
            PlanLevel::XHigh => Some(50),
            PlanLevel::Max => None,
        }
    }

    /// Ab dieser Zahl von Aufrufen hält eine Plan-Runde an (*Weiter* gibt noch einmal so viele frei – und ebenso viel
    /// Recherche-Budget): das Budget plus ein paar Aufrufe, damit das Modell die Meldung `plan_budget` sieht und den Plan
    /// schreibt. Ersetzt die Höchstzahl des Modells. *max*: keine Grenze.
    pub fn step_limit(self) -> u32 {
        match self.research_budget() {
            Some(b) => b + BUDGET_GRACE,
            None => u32::MAX,
        }
    }
}

/// So viele abgewiesene Aufrufe nach dem Recherche-Budget, bevor die Runde anhält.
pub const BUDGET_GRACE: u32 = 3;

/// Stand des aktuellen Plans.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    /// Wartet auf die Entscheidung des Nutzers (*Ausführen* / *Änderungen anfragen* / *Verwerfen*).
    #[default]
    Proposed,
    /// Der Nutzer hat Änderungen angefragt; das Modell schreibt den Plan neu.
    Revising,
    /// Freigegeben (*Ausführen*): Schreib-Werkzeuge sind frei, die Schritte stehen in der To-do-Liste.
    Approved,
    /// Verworfen.
    Discarded,
}

/// Der aktuelle Plan eines Chats (`chats/<id>.json` Feld `plan`, seit 3.1.0). Tolerant gelesen: Unlesbares ergibt
/// keinen Plan.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Plan {
    pub level: PlanLevel,
    pub status: PlanStatus,
    /// Text des Plans (Markdown) – genau der Inhalt der Antwort, die ihn enthält (getrimmt); daran erkennt die Oberfläche
    /// die Nachricht, die als Plan-Karte gezeichnet wird.
    pub markdown: String,
    /// Fassung: 1 = erster Plan, jede Überarbeitung nach *Änderungen anfragen* zählt hoch.
    pub revision: u32,
    /// Zeitpunkt des Plans (Sekunden seit 1970).
    pub created: u64,
    /// Zeitpunkt der Freigabe.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved: Option<u64>,
    /// Gespeicherte Markdown-Datei relativ zum Datenordner (`plans/<chat>-<zeit>.md`), nach der Freigabe.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Die Ausführung des freigegebenen Plans ist vorbei (eine neue Plan-Runde begann oder die To-do-Liste gehört nicht
    /// mehr zum Plan): der Systemprompt nennt ihn nicht mehr. Fehlt = `false`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub finished: bool,
}

impl Plan {
    /// Zählt für die Werkzeuge noch (nicht verworfen)?
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn current(&self) -> bool {
        self.status != PlanStatus::Discarded
    }
}

/// Plan tolerant lesen (für `ChatFile::plan`): ein unlesbares Objekt oder ein leerer Plan ergibt `None`. Ein Plan „in
/// Überarbeitung“ wartet nach dem Laden wieder auf die Entscheidung (die Runde dazu lief nur bis zum Ende des Hubs).
pub fn lenient_plan<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Plan>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value::<Plan>(v).ok().filter(|p| !p.markdown.trim().is_empty()).map(|mut p| {
        if p.status == PlanStatus::Revising {
            p.status = PlanStatus::Proposed;
        }
        p
    }))
}

/// Plan-Modus eines Chats zur Laufzeit (nicht gespeichert).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlanMode {
    /// Schalter *Plan* neben *Senden*: die nächste Nachricht des Nutzers geht im Plan-Modus los. Bleibt an, bis der Plan
    /// ausgeführt oder verworfen wird (oder der Nutzer ihn abschaltet).
    pub on: bool,
    /// Gewählte Stufe.
    pub level: PlanLevel,
    /// Die laufende (bzw. letzte) Runde ist eine Plan-Runde mit dieser Stufe: Schreib-Werkzeuge gesperrt,
    /// Recherche-Budget. Gilt für die ganze Runde samt *Weiter* und *Neu erzeugen*; jede neue Nachricht setzt es neu.
    pub round: Option<PlanLevel>,
    /// Aufrufe der laufenden Plan-Runde, die durch die Sperre kamen (zählen gegen das Recherche-Budget; Aufrufe mit
    /// kaputten Argumenten oder gesperrte zählen nicht).
    pub research: u32,
}

/// Bedienung der Plan-Karte (meldet die Oberfläche, führt die App aus).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanAction {
    Execute,
    /// Änderungen anfragen, mit dem Kommentar des Nutzers.
    RequestChanges(String),
    Discard,
    /// Als Markdown-Datei speichern (Dateidialog).
    Export,
}

/// Was der Systemprompt einer Runde über den Plan wissen muss.
#[derive(Clone, Debug, PartialEq)]
pub enum PlanPhase {
    /// Plan-Runde: recherchieren (nur lesen) und den Plan schreiben. `revision`: der Nutzer hat Änderungen angefragt.
    Research { level: PlanLevel, revision: bool },
    /// Der freigegebene Plan wird ausgeführt (solange noch Schritte offen sind); `steps` = seine Schritte.
    Execute { steps: Vec<String> },
}

// ---------------------------------------------------------------------------------------------------------------
// Lesend oder schreibend

/// Werkzeuge, die nur lesen oder fragen und in einer Plan-Runde laufen dürfen, obwohl sie keine Paneldeck-Werkzeuge der
/// Stufe „Lesen“ sind: die lesenden Agenten-Werkzeuge (#338), Webseiten/Websuche (fragen ohnehin), `wait_for_status`
/// (wartet nur), die Rückfrage `ask_user` (#356) und Stand bzw. Ausgabe von Hintergrund-Befehlen (`command_status`,
/// `command_output`, #355 – starten und anhalten (`run_command`, `stop_command`) gilt als schreibend).
pub const READ_ONLY_TOOLS: &[&str] = &["read_file", "list_dir", "glob", "grep", "git_status", "git_diff", "git_log", "web_fetch", "web_search", "wait_for_status", "ask_user", "command_status", "command_output"];

/// Rückfragen an den Nutzer: in einer Plan-Runde erlaubt, zählen aber nicht gegen das Recherche-Budget.
pub const QUESTION_TOOLS: &[&str] = &["ask_user"];

/// Werkzeuge, die in einer Plan-Runde immer gesperrt sind, auch wenn ihre Stufe „Lesen“ ist: `update_todos` (die
/// To-do-Liste füllt der freigegebene Plan).
pub const PLAN_LOCKED_TOOLS: &[&str] = &["update_todos"];

/// Ändert dieses Werkzeug nichts (darf es in einer Plan-Runde laufen)? Die **eine** Einteilung für den Plan-Modus:
/// [`READ_ONLY_TOOLS`] ja, [`PLAN_LOCKED_TOOLS`] nein, sonst Paneldeck-Werkzeuge der Stufe „Lesen“ ja. Alles andere –
/// Schreib-Werkzeuge, Befehle (`run_command`, auch im Hintergrund), Stufe „Steuern“, unbekannte und neue Werkzeuge –
/// gilt als schreibend. Neue lesende Werkzeuge gehören in [`READ_ONLY_TOOLS`] (oder bekommen die Stufe „Lesen“).
pub fn is_read_only(name: &str) -> bool {
    if READ_ONLY_TOOLS.contains(&name) {
        return true;
    }
    if PLAN_LOCKED_TOOLS.contains(&name) || super::agent::is_agent_tool(name) {
        return false;
    }
    crate::mcp::tool_level(name) == Some(McpLevel::Read)
}

/// Darf der Aufruf in einer Plan-Runde der Stufe `level` laufen? `research` = schon zugelassene Aufrufe der Runde,
/// `grants` = wie oft das Budget freigegeben ist (1 + *Weiter*). `Err` = Ergebnis-Nachricht für das Modell (Fehler
/// `plan_mode` bzw. `plan_budget`).
pub fn gate(level: PlanLevel, call: &ToolCall, research: u32, grants: u32) -> Result<(), (ToolStatus, String)> {
    if !is_read_only(&call.name) {
        let msg = format!(
            "Plan mode: {} changes something and is locked until the user approves the plan (Execute). Use only read-only tools now, then write the plan.",
            crate::mcp::short(&call.name, 60)
        );
        return Err((ToolStatus::Denied, super::tools::error_json("plan_mode", &msg)));
    }
    // Rückfragen an den Nutzer sind keine Recherche: sie gehen auch nach dem Budget.
    if QUESTION_TOOLS.contains(&call.name.as_str()) {
        return Ok(());
    }
    if let Some(b) = level.research_budget().filter(|b| research >= b.saturating_mul(grants.max(1))) {
        let msg = format!("Plan mode: the research budget of level {} ({b} tool calls) is used up. Write the plan now from what you found.", level.as_str());
        return Err((ToolStatus::Denied, super::tools::error_json("plan_budget", &msg)));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// Anweisungen für den Systemprompt (Englisch wie der Master-Prompt)

/// Abschnitt für den Systemprompt. `tools` = Namen der Werkzeuge, die das Modell in dieser Anfrage hat (die Rückfrage
/// `ask_user` wird nur verlangt, wenn es sie gibt).
pub fn instructions(phase: &PlanPhase, tools: &[String]) -> String {
    match phase {
        PlanPhase::Research { level, revision } => research(*level, *revision, tools),
        PlanPhase::Execute { steps } => execute(steps, tools),
    }
}

/// Kurzfassung von [`instructions`] für kleine Kontexte (zur Kurzfassung des Master-Prompts, `Variant::Compact`):
/// dieselben Regeln in wenigen Zeilen.
pub fn instructions_compact(phase: &PlanPhase, tools: &[String]) -> String {
    match phase {
        PlanPhase::Research { level, revision } => {
            let has_read = tools.iter().any(|t| is_read_only(t));
            let budget = match level.research_budget() {
                Some(n) => format!("at most {n} tool calls"),
                None => "no limit".to_string(),
            };
            let mut s = format!("## Plan mode ({})\nChange nothing now. ", level.as_str());
            if has_read {
                s.push_str(&format!("Research with read-only tools ({budget}); all other tools are locked (error plan_mode). "));
            }
            let depth = match level {
                PlanLevel::Low => "a short plan: goal, 3–7 steps, how to test",
                PlanLevel::Medium => "goal, findings, steps naming the files, risks, tests",
                PlanLevel::High => "goal, findings, steps file by file, risks and edge cases, tests, assumptions",
                PlanLevel::XHigh => "like high plus alternatives and compatibility; steps per file and function",
                PlanLevel::Max => "extremely detailed: scope, current state, changes file by file, risks, tests, alternatives, open questions, small steps",
            };
            s.push_str(&format!("Then write {depth}.\n"));
            let ask = tools.iter().any(|t| t == "ask_user");
            let how = if ask { "with ask_user" } else { "as numbered choice questions without the plan heading" };
            s.push_str(&match level {
                PlanLevel::Low | PlanLevel::Medium => format!("Ask only if something essential is unclear ({how}).\n"),
                PlanLevel::High | PlanLevel::XHigh => format!("Clarify open points first ({how}).\n"),
                PlanLevel::Max => format!("You must clarify open points first ({how}).\n"),
            });
            s.push_str("Format: first line `# Plan: <title>`, a section `## Steps` with a numbered list (fixed English markers; the rest in the user's language). Then stop – the user decides: Execute, Request changes or Discard.\n");
            if *revision {
                s.push_str("The user requested changes (last message): write the complete revised plan.\n");
            }
            s
        }
        PlanPhase::Execute { steps } => {
            let mut s = String::from("## Approved plan\nWork through the approved plan's steps in order");
            s.push_str(if tools.iter().any(|t| t == "update_todos") { "; keep update_todos current.\n" } else { ".\n" });
            for (i, st) in steps.iter().enumerate() {
                s.push_str(&format!("{}. {}\n", i + 1, crate::mcp::short(st, 100)));
            }
            s
        }
    }
}

fn research(level: PlanLevel, revision: bool, tools: &[String]) -> String {
    let mut s = format!("## Plan mode (level {})\n", level.as_str());
    s.push_str("The user switched on plan mode for this request. Do not change anything now: research first, then answer with a plan. Nothing is changed until the user approves the plan.\n");
    let read: Vec<&str> = tools.iter().map(String::as_str).filter(|t| is_read_only(t)).collect();
    if read.is_empty() {
        s.push_str("- You have no read-only tools: plan from what the user wrote and what you know.\n");
    } else {
        s.push_str(&format!("- Research with read-only tools only: {}.\n", read.join(", ")));
    }
    if tools.iter().any(|t| !is_read_only(t)) {
        s.push_str("- All other tools (editing, writing, moving or deleting files, commands, typing into terminals, changing sessions, to-dos or tickets) are locked until the user presses Execute; a call returns the error plan_mode. Do not try them.\n");
    }
    if !read.is_empty() {
        match level.research_budget() {
            Some(n) => s.push_str(&format!("- Research budget: at most {n} tool calls in total, then write the plan from what you found. This replaces the usual limit of tool calls per answer.\n")),
            None => s.push_str("- Research budget: no limit. Read everything relevant before you plan; research thoroughly.\n"),
        }
    }
    s.push_str(depth(level));
    s.push_str(questions(level, tools.iter().any(|t| t == "ask_user")));
    s.push_str("- Answer format: start the plan with the line `# Plan: <short title>` and include a section `## Steps` with a numbered list (`1. …`), one concrete step per line (what and where). When the user presses Execute, these steps become the chat's to-do list.\n");
    s.push_str("- `# Plan:` and `## Steps` are fixed markers that Paneldeck uses to recognise the plan: keep them exactly like this, in English, in every language; write the title and everything else in the user's language.\n");
    s.push_str("- Write the plan in the user's language. Then stop: do not start the work and do not ask whether to start – the user decides with the buttons Execute, Request changes or Discard.\n");
    if revision {
        s.push_str("- The user requested changes to your previous plan (their comment is the last message). Write the complete revised plan again, in the same format.\n");
    }
    s
}

/// Aufbau und Tiefe je Stufe.
fn depth(level: PlanLevel) -> &'static str {
    match level {
        PlanLevel::Low => "- Depth (low): a short plan – one sentence for the goal, 3–7 steps and one line on how to test. Read only what you really need.\n",
        PlanLevel::Medium => "- Depth (medium): sections Goal, Findings (what you looked at, with paths), Steps (about 5–12, naming the files), Risks (short) and Tests.\n",
        PlanLevel::High => "- Depth (high): sections Goal, Findings (relevant files and functions with paths), Steps (file by file where it helps), Risks and edge cases, Tests (what to run or add) and Assumptions. Read the relevant code before planning.\n",
        PlanLevel::XHigh => "- Depth (xhigh): like high, plus Alternatives (briefly, and why not) and Compatibility/rollback; steps detailed per file and function. Verify your assumptions in the code.\n",
        PlanLevel::Max => "- Depth (max): an extremely detailed plan. Sections: Goal and scope (in/out), Current state (each relevant file and what it does), Changes file by file (per file: what changes, which functions, data or format changes), Risks and mitigations, Tests (unit tests, manual checks, commands), Alternatives with pros and cons, Open questions and assumptions, and Steps (small, verifiable, in order).\n",
    }
}

/// Rückfragen je Stufe: ab *high* offene Punkte vorher klären, bei *max* Pflicht, bei *low*/*medium* nur bei Bedarf –
/// mit `ask_user` (#356), sonst als kurze Nachricht mit Auswahlfragen ohne Plan-Überschrift.
fn questions(level: PlanLevel, ask_user: bool) -> &'static str {
    match (level, ask_user) {
        (PlanLevel::Low | PlanLevel::Medium, true) => "- Questions: only if something essential is unclear, ask with ask_user (choice questions) before writing the plan; otherwise assume sensibly and say so in the plan.\n",
        (PlanLevel::High | PlanLevel::XHigh, true) => "- Questions: before writing the plan, clarify open points with ask_user – choice questions with a recommended option. Skip this only if nothing is unclear.\n",
        (PlanLevel::Max, true) => "- Questions: mandatory – before writing the plan, clarify the open points with ask_user (choice questions, one recommended option each). Write the plan only after the answers.\n",
        (PlanLevel::Low | PlanLevel::Medium, false) => "- Questions: if something essential is unclear, make a sensible assumption and list it in the plan.\n",
        (PlanLevel::High | PlanLevel::XHigh, false) => "- Questions: if open points need the user's decision, first reply only with a few numbered choice questions (options a), b), …, mark your recommendation) and without the plan heading; write the plan after the answers. Skip this if nothing is unclear.\n",
        (PlanLevel::Max, false) => "- Questions: mandatory – unless the request is fully specified, first reply only with numbered choice questions (options a), b), …, mark your recommendation) and without the plan heading; write the plan after the user's answers.\n",
    }
}

fn execute(steps: &[String], tools: &[String]) -> String {
    let mut s = String::from("## Approved plan\nThe user approved your plan (it is in the conversation) and pressed Execute. Work through its steps in order now, with the usual tools and approvals.");
    if tools.iter().any(|t| t == "update_todos") {
        s.push_str(" Keep the to-do list current with update_todos – it already holds the steps: mark the current one in_progress and finished ones completed.");
    }
    s.push_str(" If a step turns out to be wrong, say so and continue sensibly or ask.\n");
    if !steps.is_empty() {
        s.push_str("Steps:\n");
        for (i, st) in steps.iter().enumerate() {
            s.push_str(&format!("{}. {st}\n", i + 1));
        }
    }
    s
}

// ---------------------------------------------------------------------------------------------------------------
// Plan im Antworttext

/// Zeilen außerhalb von Codeblöcken (``` bzw. ~~~), mit ihrem Index: Überschriften und Listen in Code zählen nicht.
fn prose_lines(text: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut fence: Option<char> = None;
    for (i, l) in text.lines().enumerate() {
        let t = l.trim_start_matches(' ');
        let marker = (l.len() - t.len() <= 3).then(|| ['`', '~'].into_iter().find(|c| t.starts_with(&c.to_string().repeat(3)))).flatten();
        match (fence, marker) {
            (None, Some(c)) => fence = Some(c),
            (Some(f), Some(c)) if f == c => fence = None,
            (None, None) => out.push((i, l)),
            _ => {}
        }
    }
    out
}

/// Ebene, die eine ganz fett geschriebene Zeile („**Steps:**“) als Zwischenüberschrift bekommt (unter den echten).
const BOLD_LEVEL: usize = 4;

/// Überschrift (Ebene 1–3, höchstens 3 Leerzeichen eingerückt) → (Ebene, Text), sonst `None`. Eine Zeile, die ganz fett
/// ist (`**Steps:**`, `__Schritte__`), zählt als Überschrift der Ebene [`BOLD_LEVEL`].
fn heading(line: &str) -> Option<(usize, &str)> {
    let t = line.trim_start_matches(' ');
    if line.len() - t.len() > 3 {
        return None; // eingerückter Codeblock
    }
    let rest = t.trim_start_matches('#');
    let n = t.len() - rest.len();
    if (1..=3).contains(&n) && rest.starts_with([' ', '\t']) {
        return Some((n, rest.trim()));
    }
    let t = t.trim_end();
    for m in ["**", "__"] {
        let inner = t.strip_prefix(m).and_then(|r| r.strip_suffix(m).or_else(|| r.strip_suffix(&format!("{m}:")[..])));
        if let Some(inner) = inner.filter(|i| !i.trim().is_empty() && !i.contains(m)) {
            return Some((BOLD_LEVEL, inner.trim()));
        }
    }
    None
}

/// Ist der Überschriftentext genau `word` – danach nur Ende, `:`, ein Gedankenstrich oder `(`? „Plan: Titel“ ja,
/// „Plan questions“ oder „Planning“ nein.
/// Groß-/Kleinschreibung egal, Fettdruck und eine Nummer davor („3. Steps“, „2) Schritte“) zählen nicht.
fn heading_is(text: &str, word: &str) -> bool {
    let t = text.trim_matches(['*', '_']).trim().to_lowercase();
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    let t = match t[digits..].strip_prefix(['.', ')']) {
        Some(r) if digits > 0 => r.trim_start().trim_start_matches(['*', '_']).to_string(),
        _ => t,
    };
    t.strip_prefix(word).is_some_and(|r| {
        let r = r.trim_start();
        r.is_empty() || r.starts_with([':', '–', '—', '-', '('])
    })
}

/// Überschriften des Abschnitts mit den Schritten (Englisch und Deutsch; verlangt wird `## Steps`).
pub const STEPS_HEADINGS: &[&str] = &["steps", "implementation steps", "plan steps", "schritte", "umsetzungsschritte", "arbeitsschritte", "vorgehen", "umsetzung"];

fn steps_heading(text: &str) -> bool {
    STEPS_HEADINGS.iter().any(|w| heading_is(text, w))
}

/// Enthält die Antwort einen Plan? Eine Überschrift `# Plan …` (Ebene 1–3) oder ein Abschnitt `## Steps`/`## Schritte`,
/// außerhalb von Codeblöcken. Rückfragen ohne diese Überschriften sind kein Plan.
pub fn detect(content: &str) -> bool {
    prose_lines(content).into_iter().filter_map(|(_, l)| heading(l)).any(|(_, h)| heading_is(h, "plan") || steps_heading(h))
}

/// Listenpunkt der obersten Ebene (höchstens 1 Leerzeichen eingerückt; 2 und mehr = Unterpunkt): `1. text` / `1) text`
/// (nummeriert) bzw. `- text` / `* text` / `- [ ] text`.
fn list_item(line: &str) -> Option<(bool, &str)> {
    let t = line.trim_start_matches(' ');
    if line.len() - t.len() > 1 || t.starts_with('\t') {
        return None;
    }
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 && digits <= 3 {
        let rest = &t[digits..];
        if let Some(r) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return Some((true, r.trim()));
        }
    }
    let r = t.strip_prefix("- ").or_else(|| t.strip_prefix("* "))?;
    let r = r.strip_prefix("[ ] ").or_else(|| r.strip_prefix("[x] ")).or_else(|| r.strip_prefix("[X] ")).unwrap_or(r);
    Some((false, r.trim()))
}

/// Schritt säubern: Fettdruck weg, auf [`super::agent::MAX_TODO_CHARS`] gekürzt.
fn clean_step(s: &str) -> String {
    let s = s.replace("**", "").replace("__", "");
    let s = s.trim();
    let max = super::agent::MAX_TODO_CHARS;
    if s.chars().count() > max { format!("{}…", s.chars().take(max).collect::<String>()) } else { s.to_string() }
}

/// Schritte des Plans: die obersten Listenpunkte im Abschnitt `Steps`/`Schritte` (nummerierte vor Aufzählungen), ohne
/// solchen Abschnitt alle nummerierten Punkte der obersten Ebene; Codeblöcke zählen nicht. Höchstens
/// [`super::agent::MAX_TODOS`].
pub fn steps(markdown: &str) -> Vec<String> {
    let lines = prose_lines(markdown);
    let mut section: Option<(usize, usize)> = None; // (Ebene der Überschrift, erste Zeile in `lines`)
    let mut range = None;
    for (k, (_, l)) in lines.iter().enumerate() {
        if let Some((n, h)) = heading(l) {
            if let Some((lvl, from)) = section {
                if n <= lvl {
                    range = Some((from, k));
                    break;
                }
            } else if steps_heading(h) {
                section = Some((n, k + 1));
            }
        }
    }
    if range.is_none() {
        range = section.map(|(_, from)| (from, lines.len()));
    }
    let pick = |from: usize, to: usize, numbered_only: bool| -> Vec<String> {
        let items: Vec<(bool, &str)> = lines[from..to].iter().filter_map(|(_, l)| list_item(l)).filter(|(_, t)| !t.is_empty()).collect();
        let numbered = items.iter().any(|(n, _)| *n);
        items.into_iter().filter(|(n, _)| *n || (!numbered && !numbered_only)).map(|(_, t)| clean_step(t)).take(super::agent::MAX_TODOS).collect()
    };
    match range {
        Some((from, to)) => pick(from, to, false),
        None => pick(0, lines.len(), true),
    }
}

/// To-do-Liste aus den Schritten des Plans (alle offen).
pub fn todos(markdown: &str) -> Vec<ChatTodo> {
    steps(markdown).into_iter().map(|content| ChatTodo { content, status: TodoStatus::Pending }).collect()
}

// ---------------------------------------------------------------------------------------------------------------
// Speichern und Exportieren

/// Ordner der gespeicherten Pläne im Datenordner.
pub fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join("plans")
}

/// Inhalt der Markdown-Datei: eine Kopfzeile als HTML-Kommentar (Chat, Stufe, Fassung, Zeit), dann der Plan.
pub fn file_text(plan: &Plan, chat_title: &str) -> String {
    let when = plan.approved.unwrap_or(plan.created);
    let date = chrono::DateTime::from_timestamp(when as i64, 0).map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default();
    // Jede Folge von `-` wird zu einem (sonst schlösse z. B. „--->“ den Kommentar), Steuerzeichen fallen weg.
    let mut title = String::new();
    for c in chat_title.chars().filter(|c| !c.is_control()) {
        if !(c == '-' && title.ends_with('-')) {
            title.push(c);
        }
    }
    let status = if plan.status == PlanStatus::Approved { "approved" } else { "plan" };
    format!("<!-- Paneldeck plan · chat “{}” · level {} · revision {} · {status} {date} -->\n\n{}\n", crate::mcp::short(title.trim(), 80), plan.level.as_str(), plan.revision.max(1), plan.markdown.trim_end())
}

/// Freigegebenen Plan im Datenordner speichern (`plans/<chat>-<zeit>.md`, vorhandene Dateien bleiben). Ergebnis: Pfad
/// relativ zum Datenordner (mit `/`).
pub fn save(data_dir: &Path, chat: u64, plan: &Plan, chat_title: &str) -> std::io::Result<String> {
    let d = dir(data_dir);
    std::fs::create_dir_all(&d)?;
    let when = plan.approved.unwrap_or(plan.created);
    let mut name = format!("{chat}-{when}.md");
    let mut n = 2;
    while d.join(&name).exists() {
        name = format!("{chat}-{when}-{n}.md");
        n += 1;
    }
    crate::state::write_atomic(&d.join(&name), file_text(plan, chat_title).as_bytes())?;
    Ok(format!("plans/{name}"))
}

/// Vorgeschlagener Dateiname für *Exportieren*: `plan-<chat>-<zeit>.md` (nur Buchstaben, Ziffern, `-`, `_`).
pub fn export_name(chat_title: &str, plan: &Plan) -> String {
    let safe: String = chat_title.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    let safe = if safe.trim_matches('_').is_empty() { "chat".to_string() } else { safe.chars().take(60).collect() };
    format!("plan-{safe}-{}.md", plan.approved.unwrap_or(plan.created))
}

// ---------------------------------------------------------------------------------------------------------------
// Texte der Nachrichten, die die Plan-Karte sendet (in der Sprache der Oberfläche; sie stehen im Verlauf)

/// Nachricht nach *Ausführen*.
pub fn execute_message(lang: Lang) -> &'static str {
    lang.t("Plan freigegeben – bitte ausführen.", "Plan approved – please execute it.")
}

/// Nachricht nach *Änderungen anfragen*.
pub fn changes_message(lang: Lang, comment: &str) -> String {
    format!("{}\n\n{}", lang.t("Bitte ändere den Plan:", "Please change the plan:"), comment.trim())
}

// ---------------------------------------------------------------------------------------------------------------
// Zustand im Chat

impl ChatState {
    /// Markdown des aktuellen Plans (vorgeschlagen, in Überarbeitung oder freigegeben; nicht verworfen) – die Stelle,
    /// über die andere Teile (z. B. die Komprimierung des Verlaufs) den Plan wörtlich übernehmen.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn current_plan_markdown(&self) -> Option<&str> {
        self.file.plan.as_ref().filter(|p| p.current()).map(|p| p.markdown.as_str())
    }

    /// Index der Nachricht, die als Plan-Karte gezeichnet wird: die neueste Antwort mit genau dem Text des Plans.
    pub fn plan_message_index(&self) -> Option<usize> {
        let p = self.file.plan.as_ref()?;
        self.file.messages.iter().rposition(|m| m.role == Role::Assistant && m.error.is_none() && m.content.len() >= p.markdown.len() && m.content.trim() == p.markdown)
    }

    /// Ist die Nachricht `i` die des aktuellen Plans?
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_plan_message(&self, i: usize) -> bool {
        self.plan_message_index() == Some(i)
    }

    /// Der freigegebene Plan, solange er ausgeführt wird: nicht beendet, die To-do-Liste besteht genau aus seinen
    /// Schritten (sonst hat das Modell sie ersetzt), und es ist noch etwas offen.
    fn executing_plan(&self) -> Option<&Plan> {
        let p = self.file.plan.as_ref().filter(|p| p.status == PlanStatus::Approved && !p.finished)?;
        let todos = &self.file.todos;
        let same = !todos.is_empty() && todos.iter().map(|t| t.content.as_str()).eq(steps(&p.markdown).iter().map(String::as_str));
        (same && todos.iter().any(|t| t.status != TodoStatus::Completed)).then_some(p)
    }

    /// Plan-Teil des Systemprompts für die laufende Runde: Plan-Runde → Recherche; sonst, solange der freigegebene Plan
    /// ausgeführt wird → Ausführung.
    pub fn plan_phase(&self) -> Option<PlanPhase> {
        if let Some(level) = self.planning.round {
            let revision = self.file.plan.as_ref().is_some_and(|p| p.status == PlanStatus::Revising);
            return Some(PlanPhase::Research { level, revision });
        }
        self.executing_plan().map(|p| PlanPhase::Execute { steps: steps(&p.markdown) })
    }

    /// Wie [`ChatState::plan_phase`], aber für die nächste Nachricht (Schalter *Plan*) – für die Schätzung des Füllstands.
    pub fn next_plan_phase(&self) -> Option<PlanPhase> {
        if self.planning.on {
            return Some(PlanPhase::Research { level: self.planning.level, revision: false });
        }
        self.executing_plan().map(|p| PlanPhase::Execute { steps: steps(&p.markdown) })
    }

    /// Schalter *Plan* umlegen (Knopf oder Umschalt+Tab).
    pub fn toggle_plan(&mut self) {
        self.planning.on = !self.planning.on;
        self.touch();
    }

    /// Stufe wählen (schaltet den Plan-Modus ein).
    pub fn set_plan_level(&mut self, level: PlanLevel) {
        if self.planning.level != level || !self.planning.on {
            self.planning.level = level;
            self.planning.on = true;
            self.touch();
        }
    }

    /// Eine neue Runde beginnt (Senden): Plan-Runde der Stufe `plan` oder normal. Eine neue Plan-Runde beendet die
    /// Ausführung eines freigegebenen Plans; eine normale Nachricht lässt einen Plan „in Überarbeitung“ wieder warten.
    pub(super) fn plan_round_started(&mut self, plan: Option<PlanLevel>) {
        self.planning.round = plan;
        self.planning.research = 0;
        if let Some(p) = self.file.plan.as_mut() {
            match (plan.is_some(), p.status) {
                (true, PlanStatus::Approved) => p.finished = true,
                (false, PlanStatus::Revising) => p.status = PlanStatus::Proposed,
                _ => {}
            }
        }
        self.plan_sync();
    }

    /// *Neu erzeugen* (nachdem der Verlauf abgeschnitten wurde): War die abgeschnittene Antwort der vorgeschlagene Plan,
    /// ist auch die neue Antwort eine Plan-Runde seiner Stufe – auch nach einem Neustart, wenn der Chat die Stufe der
    /// letzten Runde nicht mehr kennt.
    pub(super) fn plan_regenerate(&mut self) {
        let lost = self.planning.round.is_none() && self.plan_message_index().is_none();
        if let Some(p) = self.file.plan.as_ref().filter(|p| lost && matches!(p.status, PlanStatus::Proposed | PlanStatus::Revising)) {
            self.planning.round = Some(p.level);
        }
        self.planning.research = 0;
        self.plan_sync();
    }

    /// Eine Plan-Runde ist zu Ende (`complete` = die letzte Antwort kam vollständig an): Steht in einer Antwort der Runde
    /// ein Plan (die neueste zählt – auch eine, die zugleich Werkzeuge aufrief), wird er der aktuelle (vorgeschlagen;
    /// nach *Änderungen anfragen* als neue Fassung). Sonst (Rückfragen, Fehler, Stopp) wartet ein Plan in Überarbeitung
    /// wieder auf die Entscheidung.
    pub(super) fn plan_round_ended(&mut self, complete: bool) {
        let Some(level) = self.planning.round else { return };
        let msgs = &self.file.messages;
        let from = super::logic::last_user(msgs).map_or(0, |i| i + 1).min(msgs.len());
        let found = if complete { msgs[from..].iter().rev().find(|m| m.role == Role::Assistant && m.error.is_none() && detect(&m.content)).map(|m| m.content.trim().to_string()) } else { None };
        match found {
            // Derselbe Plan noch einmal erkannt (z. B. Ende nach einem Stopp): bleibt, wie er ist.
            Some(markdown) if self.file.plan.as_ref().is_none_or(|p| p.markdown != markdown || p.status == PlanStatus::Revising) => {
                let revision = match &self.file.plan {
                    Some(p) if matches!(p.status, PlanStatus::Proposed | PlanStatus::Revising) => p.revision.max(1) + 1,
                    _ => 1,
                };
                self.file.plan = Some(Plan { level, status: PlanStatus::Proposed, markdown, revision, created: super::now_secs(), ..Plan::default() });
            }
            _ => {
                if let Some(p) = self.file.plan.as_mut().filter(|p| p.status == PlanStatus::Revising) {
                    p.status = PlanStatus::Proposed;
                }
            }
        }
        self.touch();
    }

    /// Ein vorgeschlagener Plan, dessen Nachricht nicht mehr im Verlauf steht (bearbeitet, neu erzeugt), fällt weg.
    pub(super) fn plan_sync(&mut self) {
        let open = self.file.plan.as_ref().is_some_and(|p| matches!(p.status, PlanStatus::Proposed | PlanStatus::Revising));
        if open && self.plan_message_index().is_none() {
            self.file.plan = None;
            self.touch();
        }
    }

    /// Wartet ein Plan auf die Entscheidung des Nutzers – und ist die Runde ganz zu Ende (auch keine angehaltene Runde
    /// mit wartenden Aufrufen)? Nur dann gehen die Knöpfe der Karte.
    pub fn plan_decidable(&self) -> bool {
        !self.round_open() && !self.editing && self.file.plan.as_ref().is_some_and(|p| p.status == PlanStatus::Proposed)
    }

    /// Dürfen externe Agenten gerade nichts senden (MCP `plan_pending`)? Ja, solange der Plan-Schalter an ist oder ein Plan
    /// auf die Entscheidung des Nutzers wartet – ein Agent soll *Ausführen* nicht umgehen.
    pub fn plan_blocks_agents(&self) -> bool {
        self.planning.on || self.file.plan.as_ref().is_some_and(|p| matches!(p.status, PlanStatus::Proposed | PlanStatus::Revising))
    }

    /// *Ausführen*: Plan freigeben, seine Schritte in die To-do-Liste (ohne Schritte bleibt sie, wie sie ist), Plan-Modus
    /// aus und die Nachricht `text` als normale Runde senden (Schreib-Werkzeuge frei, mit den üblichen Freigaben).
    /// `false`, wenn kein Plan wartet oder eine Runde läuft. Speichern als Datei macht die App danach ([`save`]).
    pub fn execute_plan(&mut self, text: &str, backend: &dyn ChatBackend, meta: &ChatMeta, cx: &TurnContext, repaint: Repaint) -> bool {
        if !self.plan_decidable() {
            return false;
        }
        let Some(p) = self.file.plan.as_mut() else { return false };
        p.status = PlanStatus::Approved;
        p.approved = Some(super::now_secs());
        p.finished = false;
        let list = todos(&p.markdown);
        if !list.is_empty() {
            self.set_todos(list);
        }
        self.planning.on = false;
        self.touch();
        self.send_message(text, None, backend, meta, cx, repaint)
    }

    /// *Änderungen anfragen*: der Kommentar geht als Plan-Runde (Stufe des Plans) an das Modell, der Plan wartet in
    /// Überarbeitung. `false`, wenn kein Plan wartet, der Kommentar leer ist oder eine Runde läuft.
    pub fn request_plan_changes(&mut self, text: &str, backend: &dyn ChatBackend, meta: &ChatMeta, cx: &TurnContext, repaint: Repaint) -> bool {
        if !self.plan_decidable() || text.trim().is_empty() {
            return false;
        }
        let Some(p) = self.file.plan.as_mut() else { return false };
        p.status = PlanStatus::Revising;
        let level = p.level;
        self.planning.on = true;
        self.planning.level = level;
        let sent = self.send_message(text, Some(level), backend, meta, cx, repaint);
        if let Some(p) = self.file.plan.as_mut().filter(|_| !sent) {
            p.status = PlanStatus::Proposed;
        }
        sent
    }

    /// *Verwerfen*: der Plan bleibt als Karte stehen (verworfen), der Plan-Modus geht aus. Nur bei ganz beendeter Runde
    /// (siehe [`ChatState::plan_decidable`]) – sonst liefen wartende Aufrufe einer angehaltenen Plan-Runde nach *Weiter*
    /// ohne die Sperre.
    pub fn discard_plan(&mut self) -> bool {
        if !self.plan_decidable() {
            return false;
        }
        if let Some(p) = self.file.plan.as_mut() {
            p.status = PlanStatus::Discarded;
        }
        self.planning.on = false;
        self.planning.round = None;
        self.touch();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::backend::{TurnEvent, TurnHandle};
    use crate::chat::store::{ChatFile, ChatMsg};
    use crate::chat::tools::ToolPolicy;
    use crate::chat::ToolStep;
    use crate::llm::config::LlmModel;
    use serde_json::json;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    /// Drehbuch-Backend: je Anfrage die nächste Ereignisliste (die letzte wiederholt sich); merkt sich den Plan-Teil
    /// jeder Anfrage.
    struct Script(Mutex<Vec<Vec<TurnEvent>>>, Mutex<Vec<Option<PlanPhase>>>);

    impl Script {
        fn new(turns: Vec<Vec<TurnEvent>>) -> Script {
            Script(Mutex::new(turns), Mutex::new(Vec::new()))
        }
        fn phases(&self) -> Vec<Option<PlanPhase>> {
            self.1.lock().unwrap().clone()
        }
    }

    impl ChatBackend for Script {
        fn start(&self, _: &ChatMeta, _: &[ChatMsg], cx: &TurnContext, _: Repaint) -> TurnHandle {
            self.1.lock().unwrap().push(cx.plan.clone());
            let (h, tx, _) = TurnHandle::pair();
            let mut v = self.0.lock().unwrap();
            let evs = if v.len() > 1 { v.remove(0) } else { v[0].clone() };
            for ev in evs {
                tx.send(ev).unwrap();
            }
            h
        }
    }

    fn tc(id: &str, name: &str, args: serde_json::Value) -> TurnEvent {
        TurnEvent::ToolCall(ToolCall { id: id.into(), name: name.into(), arguments: args })
    }

    const PLAN_TEXT: &str = "# Plan: Fix the build\n\n## Goal\nGreen build.\n\n## Steps\n1. Read **Cargo.toml**\n2. Fix the version\n   - details\n3. Run `cargo test`\n\n## Tests\n- cargo test";

    fn rp() -> Repaint {
        Arc::new(|| {})
    }

    fn policy(level: McpLevel) -> ToolPolicy {
        ToolPolicy::new(LlmModel { level, max_steps: 10, level_chosen: true, ..LlmModel::named("m") }, None, false)
    }

    /// Antworten abholen und die Werkzeug-Schleife wie der Hub weiterführen (Werkzeuge antworten sofort mit `ok`).
    /// Ergebnis: Namen der ausgeführten Werkzeuge.
    fn run(c: &mut ChatState, b: &Script, pol: &ToolPolicy) -> Vec<String> {
        let mut executed = Vec::new();
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(5) {
            c.poll();
            if c.streaming() {
                std::thread::sleep(Duration::from_millis(1));
                continue;
            }
            match c.tool_step(pol) {
                ToolStep::Execute(call) => {
                    executed.push(call.name.clone());
                    let (tx, rx) = std::sync::mpsc::sync_channel(1);
                    tx.send(Ok(json!({"ok": true}))).unwrap();
                    c.tool_started(rx);
                    c.poll_tools();
                }
                ToolStep::Request => c.next_request(b, &ChatMeta::default(), &TurnContext::default(), rp()),
                ToolStep::Idle => break,
            }
        }
        executed
    }

    impl ChatState {
        /// Antwort abholen, bis sie fertig ist (ohne die Werkzeug-Schleife).
        fn poll_until_done_for_tests(&mut self) {
            let t = Instant::now();
            while self.streaming() && t.elapsed() < Duration::from_secs(5) {
                self.poll();
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }

    fn send(c: &mut ChatState, b: &Script, text: &str) {
        c.draft = text.into();
        assert!(c.send(b, &ChatMeta::default(), &TurnContext::default(), rp()));
    }

    fn tool_results(c: &ChatState) -> Vec<(String, String)> {
        c.messages().iter().filter(|m| m.role == Role::Tool).map(|m| (m.tool_name.clone().unwrap_or_default(), m.content.clone())).collect()
    }

    #[test]
    fn levels_have_fixed_instructions_with_structure_budget_and_questions() {
        let tools: Vec<String> = ["list_sessions", "read_file", "grep", "edit_file", "run_command"].iter().map(|s| s.to_string()).collect();
        let mut with_ask = tools.clone();
        with_ask.push("ask_user".into());
        let mut texts = Vec::new();
        for level in PlanLevel::ALL {
            let t = instructions(&PlanPhase::Research { level, revision: false }, &tools);
            assert!(t.contains(&format!("level {}", level.as_str())), "{t}");
            assert!(t.contains("# Plan: <short title>") && t.contains("## Steps") && t.contains("fixed markers"), "Format: {t}");
            assert!(t.contains("read_file, grep") && t.contains("list_sessions"), "lesende Werkzeuge genannt: {t}");
            assert!(!t.contains("edit_file, ") && t.contains("plan_mode"), "Schreib-Werkzeuge gesperrt: {t}");
            match level.research_budget() {
                Some(n) => assert!(t.contains(&format!("at most {n} tool calls")), "{t}"),
                None => assert!(t.contains("no limit"), "{t}"),
            }
            // Ohne ask_user nie verlangt; mit ask_user ab high vorher, bei max Pflicht.
            assert!(!t.contains("ask_user"), "{t}");
            let a = instructions(&PlanPhase::Research { level, revision: false }, &with_ask);
            assert!(a.contains("ask_user"), "{a}");
            match level {
                PlanLevel::Max => assert!(a.contains("mandatory") && t.contains("mandatory")),
                PlanLevel::High | PlanLevel::XHigh => assert!(a.contains("before writing the plan, clarify") && !a.contains("mandatory")),
                _ => assert!(a.contains("only if something essential is unclear") && !a.contains("mandatory")),
            }
            texts.push(t);
        }
        assert!(texts[0].contains("short plan") && texts[0].contains("3–7 steps"));
        assert!(texts[4].contains("file by file") && texts[4].contains("Risks") && texts[4].contains("Tests") && texts[4].contains("Alternatives") && texts[4].contains("Open questions"));
        assert_eq!(texts.iter().collect::<std::collections::HashSet<_>>().len(), 5, "jede Stufe eigener Text");
        // Budgets steigen, max ohne Grenze
        let b: Vec<Option<u32>> = PlanLevel::ALL.iter().map(|l| l.research_budget()).collect();
        assert_eq!(b, [Some(5), Some(12), Some(25), Some(50), None]);
        // Überarbeitung und ohne Werkzeuge
        assert!(instructions(&PlanPhase::Research { level: PlanLevel::Low, revision: true }, &tools).contains("requested changes"));
        let none = instructions(&PlanPhase::Research { level: PlanLevel::High, revision: false }, &[]);
        assert!(none.contains("no read-only tools") && !none.contains("Research budget") && !none.contains("plan_mode"));
        // Ausführung nennt die Schritte und update_todos nur, wenn es das gibt
        let ex = instructions(&PlanPhase::Execute { steps: vec!["a".into(), "b".into()] }, &["update_todos".to_string()]);
        assert!(ex.contains("## Approved plan") && ex.contains("1. a\n2. b") && ex.contains("update_todos"));
        assert!(!instructions(&PlanPhase::Execute { steps: vec![] }, &[]).contains("update_todos"));
    }

    #[test]
    fn read_write_classification_is_in_one_place() {
        for t in ["read_file", "list_dir", "glob", "grep", "git_status", "git_diff", "git_log", "web_fetch", "web_search", "ask_user", "wait_for_status", "list_sessions", "read_terminal", "read_chat", "screenshot", "editor_read", "command_status", "command_output"] {
            assert!(is_read_only(t), "{t} liest nur");
        }
        for t in ["edit_file", "write_file", "move_file", "delete_file", "run_command", "update_todos", "send_text", "send_keys", "create_session", "close_session", "focus_session", "editor_open", "editor_edit", "start_process", "stop_process", "no_such_tool", "run_command_stop", "stop_command"] {
            assert!(!is_read_only(t), "{t} gilt als schreibend");
        }
        // Jedes Paneldeck-Werkzeug: Stufe Lesen = lesend, sonst schreibend (außer wait_for_status).
        for d in crate::mcp::tools() {
            if super::super::agent::is_agent_tool(d.name) {
                continue;
            }
            let expect = d.level == McpLevel::Read || d.name == "wait_for_status";
            assert_eq!(is_read_only(d.name), expect, "{}", d.name);
        }
    }

    #[test]
    fn gate_refuses_writes_and_calls_over_the_budget() {
        let call = |n: &str| ToolCall { id: "c".into(), name: n.into(), arguments: json!({}) };
        let (st, msg) = gate(PlanLevel::Max, &call("edit_file"), 0, 1).unwrap_err();
        assert_eq!(st, ToolStatus::Denied);
        assert!(msg.contains("\"plan_mode\"") && msg.contains("Execute"), "{msg}");
        assert!(gate(PlanLevel::Low, &call("read_file"), 4, 1).is_ok());
        let (_, msg) = gate(PlanLevel::Low, &call("read_file"), 5, 1).unwrap_err();
        assert!(msg.contains("plan_budget") && msg.contains("5 tool calls"), "{msg}");
        assert!(gate(PlanLevel::Max, &call("grep"), 10_000, 1).is_ok(), "max: keine Grenze");
        // *Weiter* gibt noch einmal dasselbe Budget frei
        assert!(gate(PlanLevel::Low, &call("read_file"), 7, 2).is_ok());
        assert!(gate(PlanLevel::Low, &call("read_file"), 10, 2).is_err());
        // Die Höchstzahl des Modells spielt in Plan-Runden keine Rolle mehr
        assert_eq!(PlanLevel::Low.step_limit(), 8);
        assert_eq!(PlanLevel::High.step_limit(), 28);
        assert_eq!(PlanLevel::Max.step_limit(), u32::MAX);
    }

    #[test]
    fn detects_plans_and_extracts_the_steps() {
        assert!(detect(PLAN_TEXT));
        assert!(detect("Intro\n\n## Plan\n1. a"));
        assert!(detect("### Schritte\n1. a"));
        assert!(!detect("Two questions first:\n1. a) yes b) no\n2. Which folder?"));
        assert!(!detect("# Planning questions\n1. a"), "Planning ist nicht Plan");
        assert!(!detect("#Plan without space"));
        assert!(!detect("## Plan questions\n1. a?"), "Plan questions ist kein Plan");
        assert!(detect("## Plan (draft)\n1. a") && detect("# Plan – tidy\n1. a"));
        assert!(!detect("Example:\n```md\n# Plan: x\n## Steps\n1. a\n```\nWhich one?"), "Überschrift im Codeblock");
        assert!(!detect("    # Plan: indented code"), "eingerückter Codeblock");
        assert!(detect("~~~\ncode\n~~~\n# Plan: after the code\n## Steps\n1. a"));
        assert_eq!(steps("# Plan\n## Steps\n1. a\n```sh\n1. not a step\n## Steps\n```\n2. b\n  - sub\n   3. nested\n## Tests\n1. t"), ["a", "b"], "Code und Unterpunkte zählen nicht");
        // Deutsche und andere Überschriften, Ebenen 1–3, Doppelpunkt, Nummer davor, fette Zeile
        for (text, want) in [
            ("# Plan: Aufräumen\n## Schritte\n1. eins\n2. zwei\n## Risiken\n1. r", vec!["eins", "zwei"]),
            ("# Plan: x\n## Vorgehen:\n1. a\n## Tests\n1. t", vec!["a"]),
            ("# Plan\n### UMSETZUNG\n- a\n- b", vec!["a", "b"]),
            ("# Plan\n## Implementation steps\n1. a\n## Risks\n1. r", vec!["a"]),
            ("# Plan\n## 3. Steps\n1. a\n## 4. Tests\n1. t", vec!["a"]),
            ("# Plan: x\n**Steps:**\n1. a\n2. b\n\n**Risks:**\n1. r", vec!["a", "b"]),
            ("# Plan: x\n## Steps\n**Part 1**\n1. a\n**Part 2**\n2. b\n## Tests\n1. t", vec!["a", "b"]),
        ] {
            assert!(detect(text), "{text}");
            assert_eq!(steps(text), want, "{text}");
        }
        assert!(detect("Kurz:\n\n## Schritte\n1. a"), "Schritte allein reicht");
        assert!(detect("**Steps:**\n1. a") && !detect("**Fragen:**\n1. a?"));
        assert_eq!(steps(PLAN_TEXT), ["Read Cargo.toml", "Fix the version", "Run `cargo test`"], "nur oberste Ebene, Fettdruck weg, bis zur nächsten Überschrift");
        // Aufzählung ohne Nummern im Abschnitt, Checkboxen
        assert_eq!(steps("# Plan\n## Steps\n- [ ] one\n- two\n## Risks\n- r"), ["one", "two"]);
        // Ohne Abschnitt: alle nummerierten Punkte
        assert_eq!(steps("# Plan: x\n1. a\n2) b\n- not a step"), ["a", "b"]);
        // Unterabschnitte gehören dazu
        assert_eq!(steps("# Plan\n## Steps\n### Part 1\n1. a\n### Part 2\n1. b\n## Tests\n1. t"), ["a", "b"]);
        let many: String = (1..=80).map(|i| format!("{i}. step {i}\n")).collect();
        assert_eq!(steps(&format!("# Plan\n## Steps\n{many}")).len(), super::super::agent::MAX_TODOS);
        let long = format!("# Plan\n## Steps\n1. {}", "x".repeat(400));
        assert!(steps(&long)[0].ends_with('…'));
        let t = todos(PLAN_TEXT);
        assert_eq!(t.len(), 3);
        assert!(t.iter().all(|t| t.status == TodoStatus::Pending));
    }

    #[test]
    fn write_calls_are_refused_in_plan_mode_and_allowed_after_execute() {
        let b = Script::new(vec![
            // Plan-Runde: erst ein schreibender, dann ein lesender Aufruf, dann der Plan
            vec![tc("w", "send_text", json!({"session": 4, "text": "x"})), tc("r", "list_sessions", json!({})), TurnEvent::Done],
            vec![TurnEvent::Delta(PLAN_TEXT.into()), TurnEvent::Done],
            // Ausführung: derselbe schreibende Aufruf wartet jetzt auf die normale Freigabe
            vec![tc("w2", "send_text", json!({"session": 4, "text": "x"})), TurnEvent::Done],
        ]);
        let pol = policy(McpLevel::Control);
        let mut c = ChatState::new(ChatFile::default(), None);
        c.set_plan_level(PlanLevel::High);
        send(&mut c, &b, "fix the build");
        assert_eq!(c.planning.round, Some(PlanLevel::High));
        assert_eq!(run(&mut c, &b, &pol), ["list_sessions"], "nur der lesende Aufruf lief");
        let res = tool_results(&c);
        assert!(res[0].0 == "send_text" && res[0].1.contains("plan_mode"), "{res:?}");
        assert_eq!(c.messages().iter().find(|m| m.tool_name.as_deref() == Some("send_text")).and_then(|m| m.status), Some(ToolStatus::Denied));
        // Der Plan ist da und wartet
        let p = c.file.plan.clone().expect("Plan erkannt");
        assert_eq!((p.status, p.level, p.revision), (PlanStatus::Proposed, PlanLevel::High, 1));
        assert_eq!(c.current_plan_markdown(), Some(PLAN_TEXT));
        assert!(c.is_plan_message(c.messages().len() - 1));
        assert!(c.planning.on, "Plan-Modus bleibt an, bis der Plan ausgeführt oder verworfen wird");
        // Plan-Teil der Anfragen: beide Anfragen der Runde waren Recherche
        assert!(b.phases().iter().all(|p| matches!(p, Some(PlanPhase::Research { level: PlanLevel::High, revision: false }))));
        // Ausführen
        assert!(c.execute_plan("go", &b, &ChatMeta::default(), &TurnContext::default(), rp()));
        assert!(!c.planning.on && c.planning.round.is_none());
        assert_eq!(c.file.plan.as_ref().map(|p| p.status), Some(PlanStatus::Approved));
        assert_eq!(c.file.todos.iter().map(|t| t.content.as_str()).collect::<Vec<_>>(), ["Read Cargo.toml", "Fix the version", "Run `cargo test`"]);
        assert_eq!(c.messages().iter().rev().find(|m| m.role == Role::User).map(|m| m.content.as_str()), Some("go"));
        assert!(run(&mut c, &b, &pol).is_empty());
        assert!(c.awaiting_approval(), "nach Ausführen: normale Freigabe statt plan_mode");
        assert!(matches!(b.phases().last(), Some(Some(PlanPhase::Execute { steps })) if steps.len() == 3));
        // Die Karte bleibt: der Plan zählt weiter, die Nachricht ist weiter die Plan-Nachricht
        assert!(c.is_plan_message(4));
    }

    #[test]
    fn discard_and_clear_history_never_release_queued_calls_of_a_paused_round() {
        // Plan-Runde hält an der Höchstzahl an, ein schreibender Aufruf wartet in der Schlange.
        let mut calls: Vec<TurnEvent> = (0..8).map(|i| tc(&format!("r{i}"), "list_sessions", json!({}))).collect();
        calls.push(tc("w", "send_text", json!({"session": 4, "text": "x"})));
        calls.push(TurnEvent::Done);
        for clear in [false, true] {
            let b = Script::new(vec![vec![TurnEvent::Delta(PLAN_TEXT.into()), TurnEvent::Done], calls.clone(), vec![TurnEvent::Delta("ok".into()), TurnEvent::Done]]);
            let pol = policy(McpLevel::Control);
            let mut c = ChatState::new(ChatFile::default(), None);
            c.set_plan_level(PlanLevel::Low);
            send(&mut c, &b, "plan");
            run(&mut c, &b, &pol);
            assert!(c.plan_decidable(), "Plan wartet");
            // Weitere Plan-Runde (Schalter noch an): sie hält an, der Plan von vorher wartet weiter
            send(&mut c, &b, "more research");
            run(&mut c, &b, &pol);
            assert!(c.paused && !c.calls.is_empty(), "angehalten mit wartenden Aufrufen");
            assert!(!c.plan_decidable(), "angehaltene Runde: keine Entscheidung über den Plan");
            assert!(!c.discard_plan() && !c.execute_plan("go", &b, &ChatMeta::default(), &TurnContext::default(), rp()));
            if clear {
                assert!(c.clear_history());
                assert!(c.calls.is_empty() && !c.paused && c.file.plan.is_none());
            }
            // Was auch immer danach kommt: der schreibende Aufruf läuft nie ohne Sperre
            c.resume();
            let ran = run(&mut c, &b, &pol);
            assert!(!ran.contains(&"send_text".to_string()), "{ran:?}");
            assert!(!c.awaiting_approval(), "nicht einmal zur Freigabe vorgelegt");
        }
    }

    #[test]
    fn plan_next_to_a_tool_call_and_regenerate_after_a_restart_keep_the_plan_mode() {
        // Plan und Werkzeugaufruf in derselben Antwort, danach nur ein kurzer Text
        let b = Script::new(vec![
            vec![TurnEvent::Delta(PLAN_TEXT.into()), tc("r", "list_sessions", json!({})), TurnEvent::Done],
            vec![TurnEvent::Delta("Done – see the plan above.".into()), TurnEvent::Done],
        ]);
        let pol = policy(McpLevel::Read);
        let mut c = ChatState::new(ChatFile::default(), None);
        c.toggle_plan();
        send(&mut c, &b, "plan");
        run(&mut c, &b, &pol);
        assert_eq!(c.plan_message_index(), Some(1), "die Antwort mit dem Plan ist die Karte");
        // Neustart: der Chat kennt die Stufe der letzten Runde nicht mehr; Neu erzeugen bleibt eine Plan-Runde
        let text = String::from_utf8(crate::chat::store::serialize_capped(&c.file, 1 << 20)).unwrap();
        let (mut f, _) = crate::chat::store::parse(&text).unwrap();
        f.messages.truncate(2); // nur Frage und Plan
        let b = Script::new(vec![vec![TurnEvent::Delta("# Plan: again\n## Steps\n1. x".into()), TurnEvent::Done]]);
        let mut c = ChatState::new(f, None);
        assert_eq!(c.planning.round, None);
        assert!(c.regenerate(&b, &ChatMeta::default(), &TurnContext::default(), rp()));
        assert_eq!(c.planning.round, Some(PlanLevel::Medium));
        run(&mut c, &b, &pol);
        assert_eq!(c.current_plan_markdown(), Some("# Plan: again\n## Steps\n1. x"));
        assert!(matches!(b.phases()[0], Some(PlanPhase::Research { .. })));
    }

    #[test]
    fn revising_never_sticks_and_identical_plans_mark_only_the_newest() {
        // Laden: „revising“ wartet wieder
        let (f, _) = crate::chat::store::parse(r##"{"version":1,"plan":{"status":"revising","markdown":"# Plan"}}"##).unwrap();
        assert_eq!(f.plan.map(|p| p.status), Some(PlanStatus::Proposed));
        // Eine normale Nachricht während „revising“ (nach einem Neustart oder Stopp) lässt den Plan wieder warten
        let b = Script::new(vec![vec![TurnEvent::Delta("plain".into()), TurnEvent::Done]]);
        let mut c = ChatState::new(ChatFile::default(), None);
        c.file.messages = vec![ChatMsg::new(Role::User, "q"), ChatMsg::new(Role::Assistant, PLAN_TEXT), ChatMsg::new(Role::User, "q2"), ChatMsg::new(Role::Assistant, PLAN_TEXT)];
        c.file.plan = Some(Plan { markdown: PLAN_TEXT.into(), status: PlanStatus::Revising, ..Plan::default() });
        assert_eq!(c.plan_message_index(), Some(3), "gleicher Text zweimal: nur die neueste ist die Karte");
        assert!(c.is_plan_message(3) && !c.is_plan_message(1));
        send(&mut c, &b, "normal");
        assert_eq!(c.file.plan.as_ref().map(|p| p.status), Some(PlanStatus::Proposed));
        run(&mut c, &b, &policy(McpLevel::Read));
        assert!(c.plan_blocks_agents(), "ein wartender Plan sperrt Agenten-Nachrichten");
        assert!(c.discard_plan());
        assert!(!c.plan_blocks_agents());
        c.toggle_plan();
        assert!(c.plan_blocks_agents(), "eingeschalteter Plan-Schalter sperrt ebenfalls");
    }

    #[test]
    fn execute_without_steps_keeps_the_todos_and_the_execution_phase_ends() {
        let b = Script::new(vec![vec![TurnEvent::Delta("ok".into()), TurnEvent::Done]]);
        let mut c = ChatState::new(ChatFile::default(), None);
        c.file.messages = vec![ChatMsg::new(Role::User, "q"), ChatMsg::new(Role::Assistant, "# Plan: no steps\nJust do it.")];
        c.file.plan = Some(Plan { markdown: "# Plan: no steps\nJust do it.".into(), ..Plan::default() });
        let old = vec![ChatTodo { content: "earlier".into(), status: TodoStatus::InProgress }];
        c.set_todos(old.clone());
        assert!(c.execute_plan("go", &b, &ChatMeta::default(), &TurnContext::default(), rp()));
        assert_eq!(c.file.todos, old, "ohne Schritte bleibt die To-do-Liste");
        assert_eq!(b.phases(), [None], "fremde To-dos: keine Ausführungs-Phase");
        // Mit Schritten: Ausführung, bis die To-do-Liste ersetzt wird oder eine neue Plan-Runde beginnt
        let mut c = ChatState::new(ChatFile::default(), None);
        c.file.plan = Some(Plan { status: PlanStatus::Approved, markdown: PLAN_TEXT.into(), ..Plan::default() });
        c.set_todos(todos(PLAN_TEXT));
        assert!(matches!(c.plan_phase(), Some(PlanPhase::Execute { .. })));
        c.set_todos(vec![ChatTodo { content: "something else".into(), status: TodoStatus::Pending }]);
        assert_eq!(c.plan_phase(), None, "ersetzte To-do-Liste beendet die Ausführung");
        c.set_todos(todos(PLAN_TEXT));
        c.toggle_plan();
        let b = Script::new(vec![vec![TurnEvent::Delta("Questions first?".into()), TurnEvent::Done]]);
        send(&mut c, &b, "new plan");
        run(&mut c, &b, &policy(McpLevel::Read));
        c.toggle_plan();
        assert!(c.file.plan.as_ref().is_some_and(|p| p.status == PlanStatus::Approved && p.finished));
        assert_eq!(c.next_plan_phase(), None, "eine neue Plan-Runde beendete die Ausführung");
    }

    #[test]
    fn background_commands_questions_and_events_in_a_plan_round() {
        // Hintergrund-Befehl starten und anhalten: gesperrt; Stand/Ausgabe lesen: erlaubt; Rückfrage: auch nach dem Budget
        let mut calls = vec![
            tc("bg", "run_command", json!({"command": "npm run dev", "background": true})),
            tc("st", "stop_command", json!({"id": "bg1"})),
        ];
        calls.extend((0..5).map(|i| tc(&format!("r{i}"), "list_sessions", json!({"include_exited": i % 2 == 0}))));
        calls.push(tc("q", "ask_user", json!({"questions": [{"question": "Which database?", "options": [{"label": "SQLite"}, {"label": "Postgres"}]}]})));
        calls.push(TurnEvent::Done);
        let b = Script::new(vec![calls, vec![TurnEvent::Delta(PLAN_TEXT.into()), TurnEvent::Done]]);
        let pol = policy(McpLevel::Control);
        let mut c = ChatState::new(ChatFile::default(), None);
        c.set_plan_level(PlanLevel::Low);
        send(&mut c, &b, "plan");
        // Ein früherer Hintergrund-Befehl meldet sich mitten in der Plan-Runde
        c.push_event(crate::chat::store::ChatEvent { kind: "background_command".into(), id: "bg9".into(), command: "cargo build".into(), state: "exited".into(), exit_code: Some(0), ..Default::default() }, "bg9 finished".into());
        let ran = run(&mut c, &b, &pol);
        assert!(!ran.iter().any(|n| n == "run_command" || n == "stop_command"), "{ran:?}");
        assert!(ran.contains(&"ask_user".to_string()), "Rückfrage läuft auch nach 5 Lesezugriffen: {ran:?}");
        assert_eq!(ran.iter().filter(|n| *n == "list_sessions").count(), 5);
        let res = tool_results(&c);
        // `stop_command` ohne laufende Hintergrund-Befehle: schon vor der Sperre `not_available` – gelaufen ist es nie.
        assert!(res.iter().any(|(n, r)| n == "run_command" && r.contains("plan_mode")) && res.iter().any(|(n, r)| n == "stop_command" && r.contains("\"error\"")), "{res:?}");
        assert!(c.messages().iter().any(|m| m.event.is_some()), "die Nachricht steht im Verlauf");
        assert!(b.phases().iter().all(|p| matches!(p, Some(PlanPhase::Research { .. }))), "auch die Anfrage nach der Nachricht ist eine Plan-Runde");
        assert_eq!(c.current_plan_markdown(), Some(PLAN_TEXT), "der Plan kam trotz der Nachricht an");
        assert!(c.plan_decidable());
        // Eine Nachricht, die erst nach der Runde zur Anfrage führt, sperrt die Knöpfe, bis die Runde wirklich zu Ende ist
        let b = Script::new(vec![vec![TurnEvent::Delta(PLAN_TEXT.into()), TurnEvent::Done], vec![TurnEvent::Delta("ok".into()), TurnEvent::Done]]);
        let mut c = ChatState::new(ChatFile::default(), None);
        c.toggle_plan();
        send(&mut c, &b, "plan");
        c.push_event(crate::chat::store::ChatEvent { kind: "background_command".into(), id: "bg1".into(), ..Default::default() }, "bg1 finished".into());
        c.poll_until_done_for_tests();
        assert!(c.file.plan.is_some() && c.events_pending());
        assert!(!c.plan_decidable(), "solange die Nachricht noch eine Anfrage auslöst, keine Entscheidung");
        run(&mut c, &b, &pol);
        assert!(c.plan_decidable());
    }

    /// Größe der Plan-Anweisungen: die volle Fassung bleibt unter 1000 Tokens, die Kurzfassung (für kleine Kontexte)
    /// unter 300. Mit der Kurzfassung des Master-Prompts passt eine Plan-Runde bei Lesen und bei Steuern ohne
    /// Expertenmodus samt allen Werkzeugen weiter in 8192 Tokens mit einem Fünftel Platz; Steuern im Expertenmodus mit
    /// allen Werkzeugen war schon vorher randvoll (siehe `master_prompt`) – dort zeigt der Test nur die Zahlen.
    #[test]
    fn plan_instructions_fit_into_small_contexts() {
        use crate::chat::master_prompt::{minimal_specs, Variant};
        let tok = crate::llm::tokens::estimate_tokens;
        let all: Vec<String> = ["read_file", "grep", "list_sessions", "edit_file", "run_command", "ask_user", "update_todos"].iter().map(|s| s.to_string()).collect();
        for level in PlanLevel::ALL {
            let full = tok(&instructions(&PlanPhase::Research { level, revision: true }, &all));
            let short = tok(&instructions_compact(&PlanPhase::Research { level, revision: true }, &all));
            eprintln!("PLAN {level:?}: full {full}, compact {short}");
            assert!(full < 1000 && short < 300 && short < full, "{level:?}: {full} / {short}");
        }
        let budget = 8192 * 3 / 4;
        let root = std::path::PathBuf::from(if cfg!(windows) { "C:\\work\\demo" } else { "/work/demo" });
        let env = crate::chat::agent::AgentEnv { root: Some(root), background: true, ..Default::default() };
        for (level, expert) in [(McpLevel::Read, false), (McpLevel::Read, true), (McpLevel::Control, false), (McpLevel::Control, true)] {
            let p = ToolPolicy::new(LlmModel { level, level_chosen: true, ..LlmModel::named("m") }, None, expert).with_agent(env.clone()).with_editor_switches(crate::editor_svc::settings::DOCS_OPEN | 7);
            for phase in [None, Some(PlanPhase::Research { level: PlanLevel::Max, revision: true })] {
                let cx = TurnContext { plan: phase.clone(), ..TurnContext::default() };
                let sys = tok(&crate::chat::llm_backend::system_prompt_variant(&p, "qwen3:8b", &cx, Variant::Compact)) + 4;
                let n = sys + tok(&serde_json::to_string(&minimal_specs(p.specs())).unwrap());
                eprintln!("COMPACT+PLAN {level:?} expert={expert} plan={}: {n} of {budget}, room {}", phase.is_some(), budget as i64 - n as i64);
                if !(level == McpLevel::Control && expert) {
                    assert!(n + budget / 5 <= budget, "{level:?} {expert}: {n} von {budget}");
                }
            }
        }
    }

    #[test]
    fn research_budget_refuses_further_reads() {
        let calls: Vec<TurnEvent> = (0..7).map(|i| tc(&format!("r{i}"), "list_sessions", json!({}))).chain([TurnEvent::Done]).collect();
        let b = Script::new(vec![calls, vec![TurnEvent::Delta(PLAN_TEXT.into()), TurnEvent::Done]]);
        let mut c = ChatState::new(ChatFile::default(), None);
        c.set_plan_level(PlanLevel::Low);
        send(&mut c, &b, "plan");
        assert_eq!(run(&mut c, &b, &policy(McpLevel::Read)).len(), 5, "Budget low = 5");
        let budget: Vec<_> = tool_results(&c).into_iter().filter(|(_, r)| r.contains("plan_budget")).collect();
        assert_eq!(budget.len(), 2);
        assert!(c.file.plan.is_some(), "danach schrieb das Modell den Plan");
        // Kaputte Argumente zählen nicht gegen das Budget
        let mut calls: Vec<TurnEvent> = (0..3).map(|i| tc(&format!("b{i}"), "read_terminal", json!({"lines": 5 + i}))).collect();
        calls.extend((0..5).map(|i| tc(&format!("ok{i}"), "list_sessions", json!({"include_exited": i % 2 == 0}))));
        calls.push(TurnEvent::Done);
        let b = Script::new(vec![calls, vec![TurnEvent::Delta(PLAN_TEXT.into()), TurnEvent::Done]]);
        let mut c = ChatState::new(ChatFile::default(), None);
        c.set_plan_level(PlanLevel::Low);
        send(&mut c, &b, "plan");
        assert_eq!(run(&mut c, &b, &policy(McpLevel::Read)).len(), 5, "3× bad_args, danach volle 5 Aufrufe");
        assert!(tool_results(&c).iter().filter(|(_, r)| r.contains("bad_args")).count() == 3 && !tool_results(&c).iter().any(|(_, r)| r.contains("plan_budget")));
        // Viele Aufrufe: Budget aus, nach ein paar Ablehnungen hält die Runde an; *Weiter* gibt neues Budget
        let calls: Vec<TurnEvent> = (0..12).map(|i| tc(&format!("m{i}"), "list_sessions", json!({"include_exited": i % 2 == 0}))).chain([TurnEvent::Done]).collect();
        let b = Script::new(vec![calls, vec![TurnEvent::Delta(PLAN_TEXT.into()), TurnEvent::Done]]);
        let mut c = ChatState::new(ChatFile::default(), None);
        c.set_plan_level(PlanLevel::Low);
        send(&mut c, &b, "plan");
        assert_eq!(run(&mut c, &b, &policy(McpLevel::Read)).len(), 5);
        assert!(c.paused, "nach Budget + 3 angehalten (statt nach der Höchstzahl des Modells)");
        assert_eq!(c.steps, 8);
        assert!(c.resume());
        assert_eq!(run(&mut c, &b, &policy(McpLevel::Read)).len(), 4, "Weiter: noch einmal Budget für die übrigen Aufrufe");
        // Ohne Plan-Modus gilt die normale Höchstzahl und kein Budget
        let calls: Vec<TurnEvent> = (0..7).map(|i| tc(&format!("n{i}"), "list_sessions", json!({}))).chain([TurnEvent::Done]).collect();
        let b = Script::new(vec![calls, vec![TurnEvent::Delta("done".into()), TurnEvent::Done]]);
        let mut c = ChatState::new(ChatFile::default(), None);
        send(&mut c, &b, "no plan");
        assert_eq!(run(&mut c, &b, &policy(McpLevel::Read)).len(), 7);
        assert!(c.file.plan.is_none() && b.phases().iter().all(Option::is_none));
    }

    #[test]
    fn request_changes_revises_and_discard_ends_the_plan_mode() {
        let b = Script::new(vec![
            vec![TurnEvent::Delta(PLAN_TEXT.into()), TurnEvent::Done],
            vec![TurnEvent::Delta("Which folder? a) src b) lib".into()), TurnEvent::Done],
            vec![TurnEvent::Delta("# Plan: v2\n## Steps\n1. only one".into()), TurnEvent::Done],
        ]);
        let pol = policy(McpLevel::Read);
        let mut c = ChatState::new(ChatFile::default(), None);
        assert!(!c.discard_plan() && !c.execute_plan("x", &b, &ChatMeta::default(), &TurnContext::default(), rp()), "ohne Plan nichts zu entscheiden");
        c.toggle_plan();
        assert!(c.planning.on && c.planning.level == PlanLevel::Medium, "Standardstufe medium");
        send(&mut c, &b, "plan it");
        run(&mut c, &b, &pol);
        assert!(c.plan_decidable());
        // Leerer Kommentar sendet nichts
        assert!(!c.request_plan_changes("  ", &b, &ChatMeta::default(), &TurnContext::default(), rp()));
        assert!(c.request_plan_changes("use src", &b, &ChatMeta::default(), &TurnContext::default(), rp()));
        assert_eq!(c.file.plan.as_ref().map(|p| p.status), Some(PlanStatus::Revising));
        assert!(!c.plan_decidable(), "während der Überarbeitung keine Knöpfe");
        run(&mut c, &b, &pol);
        // Das Modell fragte nur nach: der alte Plan wartet wieder
        assert_eq!(c.file.plan.as_ref().map(|p| (p.status, p.revision)), Some((PlanStatus::Proposed, 1)));
        assert!(matches!(b.phases()[1], Some(PlanPhase::Research { revision: true, .. })));
        send(&mut c, &b, "a");
        run(&mut c, &b, &pol);
        let p = c.file.plan.clone().unwrap();
        assert_eq!((p.status, p.revision, p.markdown.as_str()), (PlanStatus::Proposed, 2, "# Plan: v2\n## Steps\n1. only one"));
        assert!(!c.is_plan_message(1), "die alte Fassung ist keine Karte mehr");
        assert!(c.discard_plan());
        assert_eq!(c.file.plan.as_ref().map(|p| p.status), Some(PlanStatus::Discarded));
        assert!(!c.planning.on && c.current_plan_markdown().is_none());
        assert!(c.file.todos.is_empty(), "verworfen füllt keine To-dos");
        assert!(!c.discard_plan(), "nur einmal");
    }

    #[test]
    fn agent_messages_never_run_in_plan_mode_and_stop_reverts_a_revision() {
        let b = Script::new(vec![vec![tc("w", "send_text", json!({"session": 4, "text": "x"})), TurnEvent::Done], vec![TurnEvent::Delta("ok".into()), TurnEvent::Done]]);
        let mut c = ChatState::new(ChatFile::default(), None);
        c.toggle_plan();
        assert!(c.send_text("from an agent", &b, &ChatMeta::default(), &TurnContext::default(), rp()));
        assert_eq!(c.planning.round, None, "Agenten-Nachrichten sind nie Plan-Runden");
        run(&mut c, &b, &policy(McpLevel::Control));
        assert!(c.awaiting_approval());
        c.stop();
        // Stopp während einer Überarbeitung: der Plan wartet wieder
        let mut c = ChatState::new(ChatFile::default(), None);
        c.file.messages = vec![ChatMsg::new(Role::User, "q"), ChatMsg::new(Role::Assistant, PLAN_TEXT)];
        c.file.plan = Some(Plan { markdown: PLAN_TEXT.into(), revision: 1, ..Plan::default() });
        let slow = crate::chat::EchoBackend { delay: Duration::from_millis(50) };
        assert!(c.request_plan_changes("more", &slow, &ChatMeta::default(), &TurnContext::default(), rp()));
        c.stop();
        assert_eq!(c.file.plan.as_ref().map(|p| p.status), Some(PlanStatus::Proposed));
        // Bearbeiten der Frage entfernt die Plan-Nachricht: der vorgeschlagene Plan fällt weg
        let b = Script::new(vec![vec![TurnEvent::Delta("plain".into()), TurnEvent::Done]]);
        let mut c = ChatState::new(ChatFile::default(), None);
        c.file.messages = vec![ChatMsg::new(Role::User, "q"), ChatMsg::new(Role::Assistant, PLAN_TEXT)];
        c.file.plan = Some(Plan { markdown: PLAN_TEXT.into(), revision: 1, ..Plan::default() });
        assert!(c.begin_edit());
        c.draft = "other".into();
        assert!(c.send(&b, &ChatMeta::default(), &TurnContext::default(), rp()));
        assert!(c.file.plan.is_none());
    }

    #[test]
    fn chat_file_round_trip_and_old_files_load() {
        // Datei ohne `plan` (3.0.0) lädt wie bisher
        let (f, skipped) = crate::chat::store::parse(r#"{"version":1,"messages":[{"role":"user","content":"hi","ts":1}]}"#).unwrap();
        assert_eq!((skipped, f.plan.clone()), (0, None));
        assert!(!String::from_utf8(crate::chat::store::serialize_capped(&f, 1 << 20)).unwrap().contains("\"plan\""), "kein Plan → kein Feld");
        // Mit Plan: gespeichert und wieder geladen
        let mut f = ChatFile::default();
        f.messages.push(ChatMsg::new(Role::Assistant, PLAN_TEXT));
        f.plan = Some(Plan { level: PlanLevel::XHigh, status: PlanStatus::Approved, markdown: PLAN_TEXT.into(), revision: 2, created: 10, approved: Some(20), file: Some("plans/1-20.md".into()), finished: true });
        let text = String::from_utf8(crate::chat::store::serialize_capped(&f, 1 << 20)).unwrap();
        assert!(text.contains("\"level\":\"xhigh\"") && text.contains("\"status\":\"approved\""), "{text}");
        let (back, _) = crate::chat::store::parse(&text).unwrap();
        assert_eq!(back, f);
        // Unlesbarer oder leerer Plan: kein Plan, der Rest bleibt
        let (f, _) = crate::chat::store::parse(r#"{"version":1,"plan":{"status":"someday","markdown":"x"},"messages":[{"role":"user","content":"hi"}]}"#).unwrap();
        assert!(f.plan.is_none() && f.messages.len() == 1);
        let (f, _) = crate::chat::store::parse(r#"{"version":1,"plan":{"markdown":"  "}}"#).unwrap();
        assert!(f.plan.is_none());
        let (f, _) = crate::chat::store::parse(r##"{"version":1,"plan":{"markdown":"# Plan"}}"##).unwrap();
        assert_eq!(f.plan.map(|p| (p.level, p.status)), Some((PlanLevel::Medium, PlanStatus::Proposed)), "fehlende Felder: Standard");
        // Verlauf leeren nimmt den Plan mit
        let mut c = ChatState::new(back, None);
        assert!(c.clear_history());
        assert!(c.file.plan.is_none());
    }

    #[test]
    fn approved_plans_are_saved_as_markdown_and_exportable() {
        let d = std::env::temp_dir().join(format!("pd-plan-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let p = Plan { level: PlanLevel::Max, status: PlanStatus::Approved, markdown: PLAN_TEXT.into(), revision: 3, created: 100, approved: Some(1_763_200_000), ..Plan::default() };
        let rel = save(&d, 7, &p, "My chat").unwrap();
        assert_eq!(rel, "plans/7-1763200000.md");
        let text = std::fs::read_to_string(d.join(&rel)).unwrap();
        assert!(text.starts_with("<!-- Paneldeck plan · chat “My chat” · level max · revision 3 · approved "), "{text}");
        assert!(text.ends_with(&format!("{PLAN_TEXT}\n")));
        // Eine zweite Freigabe in derselben Sekunde überschreibt nichts
        assert_eq!(save(&d, 7, &p, "x").unwrap(), "plans/7-1763200000-2.md");
        assert_eq!(export_name("My chat: build/fix", &p), "plan-My_chat__build_fix-1763200000.md");
        assert_eq!(export_name("///", &p), "plan-chat-1763200000.md");
        for t in ["a --> b", "a ---> b", "a -- -> b", "x----"] {
            assert_eq!(file_text(&p, t).matches("-->").count(), 1, "Titel {t:?} bricht den Kommentar nicht auf");
            let head = file_text(&p, t).lines().next().unwrap().to_string();
            assert!(!head["<!--".len()..head.len() - "-->".len()].contains("--"), "{t:?}: {head}");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn next_phase_follows_the_toggle_for_the_estimate() {
        let mut c = ChatState::new(ChatFile::default(), None);
        assert_eq!(c.next_plan_phase(), None);
        c.set_plan_level(PlanLevel::Max);
        assert_eq!(c.next_plan_phase(), Some(PlanPhase::Research { level: PlanLevel::Max, revision: false }));
        c.toggle_plan();
        assert_eq!(c.next_plan_phase(), None);
        // Freigegebener Plan mit offenen To-dos: Ausführung; alles erledigt: nichts mehr
        c.file.plan = Some(Plan { status: PlanStatus::Approved, markdown: PLAN_TEXT.into(), ..Plan::default() });
        c.set_todos(todos(PLAN_TEXT));
        assert!(matches!(c.next_plan_phase(), Some(PlanPhase::Execute { .. })));
        let done: Vec<ChatTodo> = c.file.todos.iter().map(|t| ChatTodo { status: TodoStatus::Completed, ..t.clone() }).collect();
        c.set_todos(done);
        assert_eq!(c.next_plan_phase(), None);
        assert_eq!(c.plan_phase(), None);
    }
}
