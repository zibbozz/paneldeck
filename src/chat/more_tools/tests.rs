//! Tests für das Nachladen von Werkzeugen (#393): Liste und Karte folgen den Regeln, nicht gelistete Werkzeuge laufen
//! durch dieselbe Prüfung, Plan-Modus und Freigaben gelten unverändert, die volle Fassung bleibt, wie sie war.

use super::*;
use crate::chat::master_prompt::{self, Variant};
use crate::chat::plan::{self, PlanLevel};
use crate::chat::store::ToolCall;
use crate::editor_svc::settings::DOCS_OPEN;
use crate::llm::config::LlmModel;
use crate::mcp::McpLevel;

fn env() -> agent::AgentEnv {
    agent::AgentEnv { root: Some(std::path::PathBuf::from(if cfg!(windows) { "C:\\work\\demo" } else { "/work/demo" })), ..Default::default() }
}

fn policy(level: McpLevel, expert: bool) -> ToolPolicy {
    ToolPolicy::new(LlmModel { level, level_chosen: true, ..LlmModel::named("m") }, None, expert).with_agent(env()).with_editor_switches(0)
}

/// Wie [`policy`], mit allen Standard-aus-Werkzeugen angehakt (`tools::CHAT_DEFAULT_OFF`).
fn checked(level: McpLevel, expert: bool) -> ToolPolicy {
    let mut p = policy(level, expert);
    p.cfg.enabled_tools = crate::chat::tools::CHAT_DEFAULT_OFF.iter().map(|s| s.to_string()).collect();
    p
}

fn call(name: &str, args: Value) -> ToolCall {
    ToolCall { id: "c".into(), name: name.into(), arguments: args }
}

fn names(v: &[ToolSpec]) -> Vec<String> {
    v.iter().map(|s| s.name.clone()).collect()
}

/// Die Liste von `more_tools` (nur die Namen vor dem Doppelpunkt).
fn list(p: &ToolPolicy) -> Vec<String> {
    let v = run(p, &json!({})).unwrap();
    v["tools"].as_array().unwrap().iter().map(|t| t.as_str().unwrap().split(':').next().unwrap().to_string()).collect()
}

#[test]
fn the_compact_list_is_the_core_plus_the_meta_tool_and_nothing_gets_lost() {
    for (level, expert) in [(McpLevel::Read, false), (McpLevel::Read, true), (McpLevel::Control, false), (McpLevel::Control, true)] {
        let p = policy(level, expert).with_editor_switches(DOCS_OPEN | 7);
        let all = names(&p.specs());
        let shown = names(&p.listed_specs());
        let rest = names(&deferred(&p));
        assert_eq!(shown.last().map(String::as_str), Some(NAME), "{level:?} {expert}: more_tools am Ende");
        assert!(shown.iter().all(|n| n == NAME || is_core(n)), "{level:?} {expert}: nur der Kern: {shown:?}");
        // Gelistet + nachzuladen = genau die erlaubten Werkzeuge (nichts doppelt, nichts zusätzlich).
        let mut both: Vec<String> = shown.iter().filter(|n| *n != NAME).chain(rest.iter()).cloned().collect();
        both.sort();
        let mut want = all.clone();
        want.sort();
        assert_eq!(both, want, "{level:?} {expert}");
        assert!(!all.iter().any(|n| n == NAME), "specs (volle Fassung) ohne more_tools");
        assert_eq!(list(&p), rest, "{level:?} {expert}: die Liste nennt genau die nicht gelisteten");
    }
    // Ohne Werkzeuge: nichts, auch kein more_tools.
    assert!(policy(McpLevel::Off, false).listed_specs().is_empty());
    // Der Kern besteht nur aus echten Werkzeugen.
    for n in CORE {
        assert!(exists(n), "{n}");
    }
}

#[test]
fn the_list_names_only_tools_the_level_mode_switches_and_selection_allow() {
    let read = list(&policy(McpLevel::Read, false));
    assert!(read.contains(&"screenshot".to_string()) && read.contains(&"search_history".to_string()), "{read:?}");
    assert!(!read.iter().any(|n| crate::chat::tools::NOT_FOR_CHATS.contains(&n.as_str())), "nie: {read:?}");
    // Zeit und Kalender (#164): standardmäßig aus (#393) – erst angehakt lesende ab Stufe Lesen, schreibende bei Steuern
    for t in crate::chat::tools::CHAT_DEFAULT_OFF {
        assert!(!read.contains(&t.to_string()) && !list(&policy(McpLevel::Control, true)).contains(&t.to_string()), "standardmäßig aus: {t}");
    }
    let read_checked = list(&checked(McpLevel::Read, false));
    for t in ["list_alarms", "list_calendar_events", "list_save_actions"] {
        assert!(read_checked.contains(&t.to_string()), "Lesen, angehakt: {t}");
    }
    for t in ["create_alarm", "create_calendar_event"] {
        assert!(!read_checked.contains(&t.to_string()), "Lesen: kein {t}");
        assert!(list(&checked(McpLevel::Control, false)).contains(&t.to_string()), "Steuern, angehakt: {t}");
    }
    for w in ["rename_session", "close_session", "move_file", "add_todo", "send_text"] {
        assert!(!read.contains(&w.to_string()), "Lesen: kein {w}");
    }
    let ctl = list(&policy(McpLevel::Control, false));
    assert!(ctl.contains(&"rename_session".to_string()) && ctl.contains(&"close_session".to_string()) && ctl.contains(&"move_file".to_string()));
    assert!(!ctl.contains(&"create_ticket".to_string()), "einfacher Modus: keine Tickets");
    assert!(!ctl.iter().any(|n| is_core(n)), "Kern-Werkzeuge stehen schon in der Liste der Anfrage");
    assert!(list(&policy(McpLevel::Control, true)).contains(&"create_ticket".to_string()));
    // abgewählt
    let mut p = policy(McpLevel::Control, false);
    p.cfg.disabled_tools = vec!["rename_session".into(), "screenshot".into()];
    let l = list(&p);
    assert!(!l.contains(&"rename_session".to_string()) && !l.contains(&"screenshot".to_string()) && l.contains(&"focus_session".to_string()));
    // Editor-Schalter: ohne offene Dokumente nichts zu lesen, schreibend nur mit Schalter
    assert!(!list(&policy(McpLevel::Control, false)).iter().any(|n| n.starts_with("editor_")));
    let ed = list(&policy(McpLevel::Control, false).with_editor_switches(DOCS_OPEN | 2));
    assert!(ed.contains(&"editor_read".to_string()) && ed.contains(&"editor_edit".to_string()) && !ed.contains(&"editor_save".to_string()), "{ed:?}");
    // Agenten-Werkzeuge nur mit Projektordner
    let no_root = ToolPolicy::new(LlmModel { level: McpLevel::Control, level_chosen: true, ..LlmModel::named("m") }, None, false).with_editor_switches(0);
    assert!(!list(&no_root).contains(&"move_file".to_string()));
    // Jede Zeile: Name und kurzer Zweck
    let v = run(&policy(McpLevel::Read, false), &json!({})).unwrap();
    assert!(v["tools"].as_array().unwrap().iter().any(|t| t == "screenshot: Screenshot of the window or of one session."), "{v}");
    assert!(v["next"].as_str().unwrap().contains("more_tools with {\"name\""));
    assert_eq!(v["count"], json!(v["tools"].as_array().unwrap().len()));
}

#[test]
fn a_named_tool_returns_its_schema_approval_and_an_example() {
    let p = policy(McpLevel::Control, false);
    let v = run(&p, &json!({"name": "rename_session"})).unwrap();
    assert_eq!(v["tool"], "rename_session");
    let real = mcp::tools().into_iter().find(|d| d.name == "rename_session").unwrap();
    assert_eq!(v["parameters"]["required"], real.schema["required"], "dieselben Pflichtfelder");
    let keys = |s: &Value| s["properties"].as_object().unwrap().keys().cloned().collect::<Vec<_>>();
    assert_eq!(keys(&v["parameters"]), keys(&real.schema), "dieselben Felder");
    assert_eq!(v["approval"], "Level: Control (needs the user's approval).");
    assert!(crate::chat::repair::validate("rename_session", &real.schema, &v["example"]).is_ok(), "{}", v["example"]);
    assert!(v["next"].as_str().unwrap().contains("call rename_session"));
    // Nachsichtig beim Namen
    for n in ["`rename_session`", " Rename_Session() ", "\"rename_session\""] {
        assert_eq!(run(&p, &json!({"name": n})).unwrap()["tool"], "rename_session", "{n}");
    }
    assert_eq!(run(&p, &json!({"tool": "rename_session"})).unwrap()["tool"], "rename_session", "tool statt name");
    // Agenten-Werkzeug mit seinem eigenen Schema; ohne Freigabe-Pflicht bei „Vor Schreibzugriff fragen“ aus
    let mut quiet = policy(McpLevel::Control, false);
    quiet.cfg.ask_write = false;
    let mv = run(&quiet, &json!({"name": "move_file"})).unwrap();
    assert_eq!(mv["parameters"], agent::def("move_file").unwrap().schema);
    assert_eq!(mv["approval"], "Level: Control (runs without approval).");
    // Auch ein gelistetes Werkzeug lässt sich nachschlagen
    assert_eq!(run(&p, &json!({"name": "read_terminal"})).unwrap()["tool"], "read_terminal");
}

#[test]
fn the_meta_tool_never_describes_a_tool_the_policy_refuses() {
    let read = policy(McpLevel::Read, false);
    let (code, msg) = run(&read, &json!({"name": "rename_session"})).unwrap_err();
    assert_eq!(code, "forbidden");
    assert!(msg.contains("not available") && !msg.contains("properties"), "kein Schema: {msg}");
    let mut p = policy(McpLevel::Control, false);
    p.cfg.disabled_tools = vec!["rename_session".into()];
    assert_eq!(run(&p, &json!({"name": "rename_session"})).unwrap_err().0, "forbidden", "abgewählt");
    assert_eq!(run(&policy(McpLevel::Control, false), &json!({"name": "create_ticket"})).unwrap_err().0, "forbidden", "falscher Modus");
    assert_eq!(run(&policy(McpLevel::Control, false), &json!({"name": "editor_save"})).unwrap_err().0, "forbidden", "Schalter aus");
    assert_eq!(run(&policy(McpLevel::Control, false), &json!({"name": "format_disk"})).unwrap_err().0, "unknown_tool");
    assert_eq!(run(&policy(McpLevel::Off, false), &json!({})).unwrap_err().0, "forbidden", "Stufe Aus");
    // Geladen heißt nicht erlaubt: ein „geladenes“, aber verbotenes Werkzeug wird nicht gelistet.
    let sneaky = policy(McpLevel::Read, false).with_loaded(vec!["rename_session".into(), "send_text".into()]);
    assert!(!names(&sneaky.listed_specs()).iter().any(|n| n == "rename_session" || n == "send_text"));
    assert_eq!(sneaky.check(&call("rename_session", json!({"session": 3, "name": "x"}))).unwrap_err().0, ToolStatus::Denied);
}

#[test]
fn calls_to_unlisted_allowed_tools_go_through_the_same_check() {
    let ctl = policy(McpLevel::Control, false);
    assert!(!names(&ctl.listed_specs()).contains(&"rename_session".to_string()), "nicht gelistet");
    // erlaubt, schreibend → Freigabe wie bei einem gelisteten Werkzeug
    assert_eq!(ctl.check(&call("rename_session", json!({"session": 3, "name": "x"}))), Ok(true));
    assert_eq!(ctl.check(&call("screenshot", json!({}))), Ok(false));
    assert_eq!(ctl.check(&call("move_file", json!({"source": "a", "destination": "b"}))), Ok(true));
    // gegen das echte Schema geprüft
    let (st, body) = ctl.check(&call("rename_session", json!({"session": 3}))).unwrap_err();
    assert!(st == ToolStatus::Error && body.contains("bad_args") && body.contains("missing required field \\\"name\\\""), "{body}");
    // nicht erlaubt → verweigert
    for (p, t, a) in [
        (policy(McpLevel::Read, false), "rename_session", json!({"session": 3, "name": "x"})),
        (policy(McpLevel::Read, false), "close_session", json!({"session": 3})),
        (policy(McpLevel::Control, false), "create_ticket", json!({"project": "PAN", "summary": "x"})),
    ] {
        let r = p.check(&call(t, a));
        assert!(matches!(r, Err((ToolStatus::Denied, _))), "{t}: {r:?}");
    }
    let mut dis = policy(McpLevel::Control, false);
    dis.cfg.disabled_tools = vec!["rename_session".into()];
    assert!(dis.check(&call("rename_session", json!({"session": 3, "name": "x"}))).unwrap_err().1.contains("forbidden"));
    // more_tools selbst: ab Lesen, ohne Freigabe; Stufe Aus verweigert; falsche Argumente → bad_args
    assert_eq!(policy(McpLevel::Read, false).check(&call(NAME, json!({}))), Ok(false));
    assert_eq!(policy(McpLevel::Control, false).check(&call(NAME, json!({"name": "rename_session"}))), Ok(false));
    assert_eq!(policy(McpLevel::Off, false).check(&call(NAME, json!({}))).unwrap_err().0, ToolStatus::Denied);
    assert!(policy(McpLevel::Read, false).check(&call(NAME, json!({"name": ["a"]}))).unwrap_err().1.contains("bad_args"));
    // Agenten-Kette: auch ein nicht gelistetes Werkzeug schreibt nicht in die Sitzung des auslösenden Agenten
    let chained = policy(McpLevel::Control, false).with_origins(vec![7]);
    assert!(chained.check(&call("close_session", json!({"session": 7}))).unwrap_err().1.contains("forbidden"));
}

#[test]
fn plan_mode_and_approvals_apply_unchanged() {
    // more_tools liest nur: in der Plan-Runde erlaubt und kein Teil des Recherche-Budgets
    assert!(plan::is_read_only(NAME) && plan::budget_free(NAME));
    assert!(plan::gate(PlanLevel::Low, &call(NAME, json!({"name": "rename_session"})), 1000, 1).is_ok());
    // ein so nachgeschlagenes schreibendes Werkzeug bleibt im Plan-Modus gesperrt, ein lesendes zählt zum Budget
    let (st, body) = plan::gate(PlanLevel::Max, &call("rename_session", json!({"session": 3, "name": "x"})), 0, 1).unwrap_err();
    assert!(st == ToolStatus::Denied && body.contains("plan_mode"), "{body}");
    assert!(plan::gate(PlanLevel::Low, &call("screenshot", json!({})), 0, 1).is_ok());
    assert!(plan::gate(PlanLevel::Low, &call("screenshot", json!({})), 1000, 1).unwrap_err().1.contains("plan_budget"));
    // Freigaben: wie bei gelisteten Werkzeugen (fragen, „Vor Schreibzugriff fragen“ aus, dauerhafte Freigabe)
    let ctl = policy(McpLevel::Control, false);
    let alarm = call("rename_session", json!({"session": 3, "name": "x"}));
    let ask = ctl.check(&alarm).unwrap();
    assert!(ctl.needs_approval(ask, &alarm, &[]));
    let mut quiet = ctl.clone();
    quiet.cfg.ask_write = false;
    assert!(!quiet.needs_approval(ask, &alarm, &[]));
    let cs = call("close_session", json!({"session": 4}));
    let rule = crate::chat::store::AllowRule::for_call(&cs);
    assert!(!ctl.needs_approval(ctl.check(&cs).unwrap(), &cs, &[rule]), "„Immer für diesen Chat“ gilt weiter");
    // more_tools fragt nie
    assert!(!ctl.needs_approval(ctl.check(&call(NAME, json!({}))).unwrap(), &call(NAME, json!({})), &[]));
}

#[test]
fn loaded_tools_come_from_successful_fetches_newest_first() {
    let mut msgs = vec![ChatMsg::new(Role::User, "set an alarm")];
    let mut add = |id: &str, args: Value, status: ToolStatus| {
        let mut a = ChatMsg::new(Role::Assistant, "");
        a.tool_calls = vec![ToolCall { id: id.into(), name: NAME.into(), arguments: args }];
        let c = a.tool_calls[0].clone();
        msgs.push(a);
        msgs.push(crate::chat::logic::tool_result(&c, status, "{\"elided\":true}".into()));
    };
    add("a", json!({}), ToolStatus::Ok);
    add("b", json!({"name": "screenshot"}), ToolStatus::Ok);
    add("c", json!({"name": "create_ticket"}), ToolStatus::Error);
    add("d", json!({"name": "Rename_Session"}), ToolStatus::Ok);
    add("e", json!({"name": "screenshot"}), ToolStatus::Ok);
    assert_eq!(loaded(&msgs), ["screenshot", "rename_session"], "neueste zuerst, ohne Doppelte, ohne Fehlschläge und Listen");
    // Höchstzahl
    let many: Vec<String> = (0..20).map(|i| format!("t{i}")).collect();
    let mut long = Vec::new();
    for (i, n) in many.iter().enumerate() {
        let mut a = ChatMsg::new(Role::Assistant, "");
        a.tool_calls = vec![ToolCall { id: format!("x{i}"), name: NAME.into(), arguments: json!({"name": n}) }];
        let c = a.tool_calls[0].clone();
        long.push(a);
        long.push(crate::chat::logic::tool_result(&c, ToolStatus::Ok, "{}".into()));
    }
    assert_eq!(loaded(&long).len(), MAX_LOADED);
    assert_eq!(loaded(&long)[0], "t19");
    // Geladen + erlaubt → gelistet (vor more_tools), in der Reihenfolge von specs
    let p = policy(McpLevel::Control, false).with_loaded(loaded(&msgs));
    let shown = names(&p.listed_specs());
    assert!(shown.contains(&"rename_session".to_string()) && shown.contains(&"screenshot".to_string()) && shown.last().unwrap() == NAME, "{shown:?}");
    assert!(!list(&p).contains(&"rename_session".to_string()), "nicht noch einmal in der Liste");
}

#[test]
fn the_full_prompt_and_its_tools_stay_as_they_were() {
    for (level, expert) in [(McpLevel::Read, false), (McpLevel::Control, true)] {
        // Standard: die Standard-aus-Werkzeuge (#393) fehlen auch in der vollen Fassung
        let d = master_prompt::render(&policy(level, expert), "qwen3:8b");
        assert!(d.contains("\n### screenshot\n") && !d.contains("\n### list_alarms\n"), "{level:?}: Zeit/Kalender standardmäßig aus");
        let p = checked(level, expert);
        assert_eq!(master_prompt::request_specs(&p, Variant::Full), p.specs(), "voll: jedes erlaubte Werkzeug direkt");
        let full = master_prompt::render(&p, "qwen3:8b");
        assert!(!full.contains(NAME), "{level:?}: kein more_tools in der vollen Fassung");
        assert!(full.contains(&format!("You have {} tools", p.specs().len())));
        // jedes erlaubte Werkzeug hat seinen Abschnitt
        assert!(full.contains("\n### screenshot\n") && full.contains("\n### list_alarms\n"), "angehakt: Zeit/Kalender direkt in der vollen Fassung");
        // die Kurzfassung nennt more_tools mit einer klaren Anleitung
        let compact = master_prompt::render_compact(&p, "qwen3:8b");
        assert!(compact.contains("more tools are not listed: call `more_tools` for their names, then `more_tools` with `name` for one tool's fields, then call that tool by its name."), "{compact}");
        let min = master_prompt::request_specs(&p, Variant::Compact);
        let mt = min.iter().find(|s| s.name == NAME).unwrap();
        assert_eq!(mt.description, PURPOSE);
        assert_eq!(mt.parameters, json!({"type": "object", "properties": {"name": {"type": "string"}}}));
    }
    // Werkzeuge der vollen Fassung = alle erlaubten; für große Kontexte gilt weiter die volle Fassung
    assert_eq!(master_prompt::choose(1 << 20, 30_000), Variant::Full);
}
