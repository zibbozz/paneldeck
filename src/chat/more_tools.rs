//! Werkzeuge nachladen (#393): Kleine Kontexte (etwa 8192 Tokens) bekommen in der Kurzfassung des Chat-Prompts nur den
//! Kern der Werkzeuge ([`CORE`]) und das Meta-Werkzeug [`NAME`] (`more_tools`). Es nennt die übrigen Werkzeuge, die die
//! Regeln gerade erlauben (Name und Zweck), und mit `name` Zweck, Felder (Schema), Freigabe und ein Beispiel eines davon;
//! danach ruft das Modell es ganz normal auf. Die volle Fassung (großer Kontext) bekommt weiter jedes Werkzeug direkt und
//! kein `more_tools` (Entscheidung des Nutzers in #393).
//!
//! **Kein Umweg an den Regeln vorbei:** Was `more_tools` nennt und beschreibt, kommt aus `ToolPolicy::specs` – genau den
//! Werkzeugen, die die volle Fassung bekäme (Stufe, Modus, abgewählte Werkzeuge, Editor- und andere Schalter,
//! Projektordner). Jeder Aufruf – gelistet, geladen oder einfach beim Namen genannt – läuft wie bisher durch
//! `ToolPolicy::check` (Stufe, echtes Schema, Umweg- und Start-Schutz), den Plan-Modus (`plan::gate`) und die Freigabe.
//! `more_tools` selbst liest nur (Stufe Lesen, ohne Freigabe, auch im Plan-Modus) und führt nichts aus.
//!
//! **Geladene Werkzeuge:** Ein gelungenes `more_tools` mit `name` setzt das Werkzeug für die folgenden Anfragen des Chats mit
//! in die Werkzeug-Liste der Anfrage ([`loaded`], aus dem Verlauf, höchstens [`MAX_LOADED`], neueste zuerst) – manche
//! Server (z. B. llama.cpp mit Grammatik) lassen nur Aufrufe gelisteter Werkzeuge zu. Der Aufruf eines erlaubten, nicht
//! gelisteten Werkzeugs gilt trotzdem (geprüft gegen sein echtes Schema).
//!
//! **Zurückgehaltene Werkzeuge (für spätere Merges):** Werkzeuge, die Chats bisher aus Platzgründen gar nicht bekamen,
//! brauchen das nicht mehr – sie erscheinen über `more_tools`, sobald `ToolPolicy::specs` sie erlaubt. Beim Zusammenführen
//! mit Zweigen, die solche Werkzeuge noch hart ausschließen, fällt dieser Ausschluss einfach weg:
//! * `editor_diff` (#385) und `list_save_actions` (#383): die Zweige `"editor_diff" => false` bzw.
//!   `"list_save_actions" => false` in `editor_svc::tools::offered_to_chats` streichen;
//! * `create_project`, `delete_ticket`, `update_comment`, `delete_comment` (#386): die Schalter in
//!   `access::offered_to_chats` bleiben; `update_comment` (ohne Schalter erlaubt) dort immer anbieten;
//!
//! Was Chats trotz allem nie bekommen sollen, steht an genau einer Stelle: `tools::NOT_FOR_CHATS` (derzeit Reiter „Zeit“
//! und Kalender, #164 – eine Entscheidung über die volle Fassung, nicht über Platz in der Kurzfassung).

use super::agent;
use super::master_prompt;
use super::store::{ChatMsg, Role, ToolStatus};
use super::tools::{ToolPolicy, ToolResult};
use crate::llm::ToolSpec;
use crate::mcp;
use serde_json::{json, Value};

/// Name des Meta-Werkzeugs.
pub const NAME: &str = "more_tools";

/// Kern der Kurzfassung: die Werkzeuge, die ein kleiner Kontext immer direkt bekommt (soweit Stufe, Modus und Schalter
/// sie erlauben). Klein halten – jedes Werkzeug kostet Platz in 8192 Tokens; alles andere kommt über [`NAME`].
pub const CORE: &[&str] = &[
    // Sitzungen und Terminals
    "whoami",
    "list_sessions",
    "read_terminal",
    "send_text",
    "send_keys",
    "wait_for_status",
    // Befehle im Terminal des Chats (#338; die Hintergrund-Werkzeuge #355 gibt es nur mit Hintergrund-Befehlen)
    "run_command",
    "command_status",
    "command_output",
    "stop_command",
    // Dateien im Projektordner
    "read_file",
    "list_dir",
    "glob",
    "grep",
    "edit_file",
    "write_file",
    // To-dos des Chats, Rückfragen (auch im Plan-Modus) und die To-do-Liste des Nutzers
    "update_todos",
    "ask_user",
    "list_todos",
    "add_todo",
    "set_todo_done",
];

/// Höchstzahl der nachgeladenen Werkzeuge, die zusätzlich in der Werkzeug-Liste der Anfrage stehen (die neuesten).
pub const MAX_LOADED: usize = 8;

/// Zweck in der knappen Beschreibung der Kurzfassung (`master_prompt::minimal_specs`: ohne `;`, höchstens 80 Zeichen).
pub const PURPOSE: &str = "List more tools, or with name get one tool's fields. Then call it by name.";

/// Beschreibung in der Werkzeug-Liste (vor dem Kürzen durch `minimal_specs`).
pub const DESCRIPTION: &str = "Lists the tools that are not in your tool definitions (name and purpose). With name: that tool's purpose, fields, approval and an example – then call that tool by its name like any other tool. Read-only.";

/// Gehört das Werkzeug zum Kern?
pub fn is_core(name: &str) -> bool {
    CORE.contains(&name)
}

/// Schema von `more_tools`: optional `name` (ein Werkzeug aus der Liste).
pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {"name": {"type": "string", "description": "One tool name from the list; leave out to get the list"}},
        "required": [],
        "additionalProperties": false
    })
}

/// Beschreibung von `more_tools` für die Anfrage.
pub fn spec() -> ToolSpec {
    ToolSpec { name: NAME.to_string(), description: DESCRIPTION.to_string(), parameters: schema() }
}

/// Werkzeuge der Kurzfassung in der Reihenfolge von `ToolPolicy::specs`: Kern und geladene ([`ToolPolicy::loaded`]),
/// dazu am Ende `more_tools`, wenn es weitere gibt. Ohne Werkzeuge (Stufe Aus) leer.
pub fn listed(policy: &ToolPolicy) -> Vec<ToolSpec> {
    let (mut shown, rest): (Vec<ToolSpec>, Vec<ToolSpec>) = policy.specs().into_iter().partition(|s| shown_directly(policy, &s.name));
    if !rest.is_empty() {
        shown.push(spec());
    }
    shown
}

/// Erlaubte Werkzeuge, die die Kurzfassung nicht direkt nennt (nur über `more_tools`).
pub fn deferred(policy: &ToolPolicy) -> Vec<ToolSpec> {
    policy.specs().into_iter().filter(|s| !shown_directly(policy, &s.name)).collect()
}

fn shown_directly(policy: &ToolPolicy, name: &str) -> bool {
    is_core(name) || policy.loaded.iter().any(|l| l == name)
}

/// Werkzeugname aus den Argumenten (`name`, nachsichtig auch `tool`), bereinigt: ohne Leerraum, Anführungszeichen und `()`,
/// klein geschrieben. `None` = die Liste.
pub fn wanted(args: &Value) -> Option<String> {
    let raw = ["name", "tool"].iter().find_map(|k| args.get(*k).and_then(Value::as_str))?;
    let t = raw.trim().trim_matches(|c: char| c == '`' || c == '"' || c == '\'' || c.is_whitespace());
    let t = t.strip_suffix("()").unwrap_or(t).trim();
    (!t.is_empty()).then(|| t.to_lowercase())
}

/// Gibt es ein Werkzeug dieses Namens überhaupt (Paneldeck oder Agenten-Werkzeug)?
fn exists(name: &str) -> bool {
    mcp::tool_level(name).is_some() || agent::is_agent_tool(name)
}

/// `more_tools` ausführen (im Chat selbst, `ChatState::tool_step`). Ohne `name` die Liste der nicht gelisteten,
/// erlaubten Werkzeuge; mit `name` die Karte eines erlaubten Werkzeugs. Ein Werkzeug, das die Regeln nicht erlauben,
/// liefert `forbidden` (ohne Schema), ein unbekannter Name `unknown_tool`.
pub fn run(policy: &ToolPolicy, args: &Value) -> ToolResult {
    if !policy.enabled() {
        return Err(("forbidden".into(), "This chat has no tools (tool level Off, Settings → Models).".into()));
    }
    let all = policy.specs();
    let Some(want) = wanted(args) else {
        let tools: Vec<String> = all.iter().filter(|s| !shown_directly(policy, &s.name)).map(|s| format!("{}: {}", s.name, master_prompt::compact_purpose(&s.name))).collect();
        let next = if tools.is_empty() {
            "All your tools are already in your tool definitions."
        } else {
            "To use one, call more_tools with {\"name\": \"<tool>\"} to get its fields, then call that tool by its name."
        };
        return Ok(json!({ "tools": tools, "count": tools.len(), "next": next }));
    };
    match all.iter().find(|s| s.name == want) {
        Some(s) => Ok(card(policy, s)),
        None if exists(&want) => Err((
            "forbidden".into(),
            format!("{want} is not available in this chat (tool level, to-do mode, a switch or the user's selection in Settings → Models). Do not call it; tell the user if it is needed."),
        )),
        None => Err(("unknown_tool".into(), format!("There is no tool named “{}”. Call more_tools without name for the list.", mcp::short(&want, 80)))),
    }
}

/// Karte eines Werkzeugs: Zweck, Freigabe, Felder (das Schema, wie es die volle Fassung bekommt), Hinweise, ein Beispiel.
fn card(policy: &ToolPolicy, s: &ToolSpec) -> Value {
    let def = if agent::is_agent_tool(&s.name) { agent::def(&s.name).map(agent::as_tool_def) } else { mcp::tools().into_iter().find(|d| d.name == s.name) };
    let doc = master_prompt::chat_doc(&s.name);
    let mut v = json!({
        "tool": s.name,
        "purpose": doc.map_or_else(|| s.description.clone(), |d| d.purpose.to_string()),
        "parameters": s.parameters,
    });
    if let Some(d) = &def {
        v["approval"] = json!(master_prompt::approval_text(d, policy.cfg.ask_write));
    }
    if let Some(d) = doc {
        if !d.usage.is_empty() {
            v["notes"] = json!(d.usage);
        }
        if let Some(ex) = d.examples.first().and_then(|(a, _)| serde_json::from_str::<Value>(a).ok()) {
            v["example"] = ex;
        }
    }
    v["next"] = json!(format!("Now call {} by its name with these fields (the ids in the example are placeholders).", s.name));
    v
}

/// Werkzeuge, die der Chat über `more_tools` geladen hat: Aufrufe mit `name`, deren Ergebnis gelang (der Name kommt aus dem
/// Aufruf, nicht aus dem – evtl. gekürzten – Ergebnis), neueste zuerst, ohne Doppelte, höchstens [`MAX_LOADED`]. Auch
/// kompaktierte Nachrichten zählen (das Modell kennt das Werkzeug aus der Zusammenfassung). Ob ein geladenes Werkzeug
/// gelistet wird, entscheidet weiter `ToolPolicy::specs` (nur erlaubte).
pub fn loaded(messages: &[ChatMsg]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (i, m) in messages.iter().enumerate().rev() {
        if m.role != Role::Tool || m.tool_name.as_deref() != Some(NAME) || m.status != Some(ToolStatus::Ok) {
            continue;
        }
        let Some(id) = m.tool_call_id.as_deref() else { continue };
        let call = messages[..i].iter().rev().filter(|a| a.role == Role::Assistant).flat_map(|a| a.tool_calls.iter()).find(|c| c.id == id && c.name == NAME);
        let Some(name) = call.and_then(|c| wanted(&c.arguments)) else { continue };
        if !out.contains(&name) {
            out.push(name);
            if out.len() >= MAX_LOADED {
                break;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests;
