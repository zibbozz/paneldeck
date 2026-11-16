//! Agenten-Ketten (#152): Sitzungen, die aus einer Agenten-Kette stammen, sind markiert.
//!
//! Jede Sitzung trägt ihre Vorfahren in `SessionMeta::agent_chain` (gespeichert in `state.json`, übersteht also einen
//! Neustart; Sitzungs-Ids werden nie neu vergeben). Ein Agent – Terminal-Agent über die MCP-Brücke, Claude-Sitzung oder
//! Chat – darf in keine Sitzung seiner Vorfahren schreiben (`write_forbidden`, geprüft in `App::mcp_target`).
//!
//! Die Kette wächst so:
//! * `create_session` (Terminal, Claude-Sitzung, Chat): die neue Sitzung bekommt die Vorfahren des Erstellers und den
//!   Ersteller selbst – immer, auch wenn der Ersteller vom Nutzer gestartet wurde (sein Kind darf nicht in ihn schreiben).
//! * Schreiben (`send_text`, `send_keys`, `run_command`, `queue_prompt`, `send_chat_message`) eines Aufrufers, der
//!   selbst Vorfahren hat (Glied einer Kette), in eine Sitzung außerhalb seiner Kette: das Ziel wird genauso markiert.
//!   Sonst könnte ein Glied der Kette einer bestehenden Claude-Sitzung einen Prompt geben, die dann für es schreibt.
//!   Ein vom Nutzer gestarteter Agent ohne Kette markiert beim Schreiben nichts (verhält sich wie bisher).
//! * Eine von einem Agenten ausgelöste Chat-Runde (`ChatState::origin_sessions`, seit 2.2.0) zählt für die Dauer der
//!   Runde zusätzlich zu den Vorfahren des Chats.
//!
//! Der Nutzer selbst ist nie eingeschränkt (Tippen, Einfügen, eigene Chat-Nachrichten gehen nicht über MCP).

use super::Entry;
use crate::chat::tools::chain_origins;
use crate::state;
use std::collections::BTreeMap;

/// Vorfahren eines Aufrufers: seine gespeicherte Kette und – ist er ein Chat – die Kette der laufenden Runde.
/// Leer für Aufrufer außerhalb von Paneldeck (`None`) und für vom Nutzer gestartete Sitzungen.
pub(super) fn ancestors(entries: &BTreeMap<u64, Entry>, caller: Option<u64>) -> Vec<u64> {
    let Some(e) = caller.and_then(|c| entries.get(&c)) else { return Vec::new() };
    let turn: &[u64] = e.chat.as_ref().map(|c| c.origin_sessions.as_slice()).unwrap_or(&[]);
    let mut v: Vec<u64> = e.meta.agent_chain.iter().chain(turn.iter()).copied().collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Kette, die ein Aufrufer weitergibt (an eine Sitzung, die er anlegt oder beschreibt, oder an eine Chat-Runde, die er
/// auslöst): seine Vorfahren und er selbst. Leer ohne Aufrufer.
pub(super) fn handed_on(entries: &BTreeMap<u64, Entry>, caller: Option<u64>) -> Vec<u64> {
    chain_origins(caller, &ancestors(entries, caller))
}

/// Darf `caller` nicht in `target` schreiben, weil `target` zu seinen Vorfahren gehört?
pub(super) fn write_forbidden(entries: &BTreeMap<u64, Entry>, caller: Option<u64>, target: u64) -> bool {
    ancestors(entries, caller).contains(&target)
}

/// Neue Sitzung `child`, angelegt von `creator`: bekommt dessen Kette samt ihm selbst. `true` = Kette geändert.
pub(super) fn on_create(entries: &mut BTreeMap<u64, Entry>, creator: Option<u64>, child: u64) -> bool {
    let add = handed_on(entries, creator);
    merge_into(entries, child, &add)
}

/// `caller` hat in `target` geschrieben: Ist er Glied einer Kette (hat Vorfahren), wird `target` mit dieser Kette
/// samt ihm markiert. `true` = Kette geändert (dann speichern).
pub(super) fn on_write(entries: &mut BTreeMap<u64, Entry>, caller: Option<u64>, target: u64) -> bool {
    if ancestors(entries, caller).is_empty() {
        return false;
    }
    let add = handed_on(entries, caller);
    merge_into(entries, target, &add)
}

/// `add` in die Kette von `target` übernehmen (sortiert, ohne Doppelte, nie die Sitzung selbst).
fn merge_into(entries: &mut BTreeMap<u64, Entry>, target: u64, add: &[u64]) -> bool {
    let Some(e) = entries.get_mut(&target) else { return false };
    let chain = &mut e.meta.agent_chain;
    let before = chain.len();
    chain.extend(add.iter().copied().filter(|s| *s != target));
    chain.sort_unstable();
    chain.dedup();
    chain.len() != before
}

/// Ketten bereinigen: Sitzungen, die es nicht mehr gibt, fallen heraus (beim Laden und nach dem Schließen einer
/// Sitzung). Sicher, weil Sitzungs-Ids nie neu vergeben werden – eine verschwundene Id kann niemand mehr sein.
/// `true` = etwas geändert.
pub(super) fn prune(entries: &mut BTreeMap<u64, Entry>) -> bool {
    let alive: std::collections::BTreeSet<u64> = entries.keys().copied().collect();
    let mut changed = false;
    for e in entries.values_mut() {
        let before = e.meta.agent_chain.len();
        e.meta.agent_chain.retain(|s| alive.contains(s));
        changed |= e.meta.agent_chain.len() != before;
        if let Some(c) = e.chat.as_mut() {
            c.origin_sessions.retain(|s| alive.contains(s));
        }
    }
    changed
}

/// Der Nutzer hebt die Markierung einer Sitzung auf (Kontextmenü „Agenten-Kette zurücksetzen“). Nur aus der
/// Oberfläche, nie über MCP. Die Kette einer gerade laufenden Chat-Runde bleibt bis zu deren Ende. `true` = geändert.
pub(super) fn reset(entries: &mut BTreeMap<u64, Entry>, id: u64) -> bool {
    entries.get_mut(&id).is_some_and(|e| !std::mem::take(&mut e.meta.agent_chain).is_empty())
}

/// Hinweis für Tooltip/Seitenleiste: „Aus Agenten-Kette gestartet: A, K“ (Namen der Vorfahren, die es noch gibt).
/// `None` für unmarkierte Sitzungen.
pub(super) fn hint(entries: &BTreeMap<u64, Entry>, id: u64) -> Option<String> {
    let chain = &entries.get(&id)?.meta.agent_chain;
    if chain.is_empty() {
        return None;
    }
    let names: Vec<String> = chain
        .iter()
        .map(|s| match entries.get(s) {
            Some(e) if !e.display_name().trim().is_empty() => e.display_name().to_string(),
            _ => format!("#{s}"),
        })
        .collect();
    Some(format!("{} {}", state::t("Aus Agenten-Kette gestartet:", "Started from agent chain:"), names.join(", ")))
}

/// Fehler, wenn ein Agent in eine Sitzung seiner Kette schreiben will.
pub(super) fn chain_error() -> (String, String) {
    (
        "forbidden".to_string(),
        state::t(
            "Ein Agent darf nicht in eine Sitzung seiner Agenten-Kette schreiben (die Sitzung, die ihn angelegt oder in ihn geschrieben hat, direkt oder über andere Sitzungen).",
            "An agent must not write into a session of its agent chain (the session that created it or wrote into it, directly or through other sessions).",
        )
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::tools::proxy_forbidden;
    use crate::chat::{self as chatmod, ChatState};
    use crate::state::{SessionKind, SessionMeta};

    fn world(ids: &[(u64, bool)]) -> BTreeMap<u64, Entry> {
        ids.iter()
            .map(|(id, chat)| {
                let meta = SessionMeta { id: *id, kind: if *chat { SessionKind::Chat } else { SessionKind::Terminal }, ..SessionMeta::default() };
                let e = if *chat { Entry::new_chat(meta, ChatState::new(chatmod::ChatFile::default(), None)) } else { Entry::new(meta) };
                (*id, e)
            })
            .collect()
    }

    /// Wie `App::chat_send` mit `ChatSendOrigin::Agent`: Kette der Runde + Markierung, wenn der Absender Glied einer Kette ist.
    fn agent_sends_chat(w: &mut BTreeMap<u64, Entry>, caller: u64, chat: u64) {
        assert!(!write_forbidden(w, Some(caller), chat));
        let turn = handed_on(w, Some(caller));
        on_write(w, Some(caller), chat);
        w.get_mut(&chat).unwrap().chat.as_mut().unwrap().origin_sessions = turn;
    }

    /// Wie `App::chat_send` mit `ChatSendOrigin::User`: die Kette der Runde fällt weg, die Markierung bleibt.
    fn user_sends_chat(w: &mut BTreeMap<u64, Entry>, chat: u64) {
        w.get_mut(&chat).unwrap().chat.as_mut().unwrap().origin_sessions = Vec::new();
    }

    #[test]
    fn agent_chat_terminal_agent_cannot_write_back_into_the_original_agent() {
        // Agent in Terminal 5 (vom Nutzer gestartet) → Chat 10 → create_session Terminal 20 → Agent dort.
        let mut w = world(&[(5, false), (10, true), (30, false)]);
        agent_sends_chat(&mut w, 5, 10);
        assert_eq!(ancestors(&w, Some(10)), [5]);
        // Chat 10 legt Terminal 20 an (wie `mcp_create`)
        w.insert(20, Entry::new(SessionMeta { id: 20, ..SessionMeta::default() }));
        assert!(on_create(&mut w, Some(10), 20));
        assert_eq!(w[&20].meta.agent_chain, [5, 10]);
        // Agent in 20 (über die Brücke im Prozessbaum von 20 erkannt): nicht in 5, nicht in 10
        assert!(write_forbidden(&w, Some(20), 5));
        assert!(write_forbidden(&w, Some(20), 10));
        assert!(!write_forbidden(&w, Some(20), 30), "Sitzung außerhalb der Kette");
        assert!(!write_forbidden(&w, Some(20), 20), "die eigene Sitzung prüft mcp_target getrennt");
        // Die Markierung bleibt, auch wenn der Nutzer danach im Chat schreibt …
        user_sends_chat(&mut w, 10);
        assert!(write_forbidden(&w, Some(20), 5));
        // … der Chat selbst ist nach einer Nutzer-Runde wieder frei (wie 2.2.0), er war ja vom Nutzer angelegt.
        assert!(ancestors(&w, Some(10)).is_empty());
        // Der ursprüngliche Agent bleibt unbeschränkt
        assert!(ancestors(&w, Some(5)).is_empty());
        for t in [10, 20, 30] {
            assert!(!write_forbidden(&w, Some(5), t), "5 → {t}");
        }
    }

    #[test]
    fn a_claude_child_cannot_write_into_its_parent_or_grandparent() {
        // Vom Nutzer gestarteter Agent 5 legt Claude-Sitzung 21 an, die legt 22 an.
        let mut w = world(&[(5, false), (21, false), (22, false)]);
        on_create(&mut w, Some(5), 21);
        on_create(&mut w, Some(21), 22);
        assert_eq!(w[&21].meta.agent_chain, [5]);
        assert_eq!(w[&22].meta.agent_chain, [5, 21]);
        assert!(write_forbidden(&w, Some(21), 5));
        assert!(write_forbidden(&w, Some(22), 5) && write_forbidden(&w, Some(22), 21));
        // Eltern dürfen ihre Kinder weiter steuern
        assert!(!write_forbidden(&w, Some(5), 21) && !write_forbidden(&w, Some(21), 22));
        // Ein Aufrufer außerhalb von Paneldeck vererbt nichts
        w.insert(23, Entry::new(SessionMeta { id: 23, ..SessionMeta::default() }));
        assert!(!on_create(&mut w, None, 23));
        assert!(w[&23].meta.agent_chain.is_empty());
    }

    #[test]
    fn a_chain_member_taints_the_session_it_writes_into() {
        // Kette 5 → 20; 20 gibt der bestehenden Claude-Sitzung 40 des Nutzers einen Prompt (queue_prompt).
        let mut w = world(&[(5, false), (20, false), (40, false), (41, false)]);
        on_create(&mut w, Some(5), 20);
        assert!(!write_forbidden(&w, Some(20), 40));
        assert!(on_write(&mut w, Some(20), 40));
        assert_eq!(w[&40].meta.agent_chain, [5, 20]);
        // 40 kann den Prompt nicht weiterreichen: weder in 20 noch in 5
        assert!(write_forbidden(&w, Some(40), 5) && write_forbidden(&w, Some(40), 20));
        // erneutes Schreiben ändert nichts mehr (kein unnötiges Speichern)
        assert!(!on_write(&mut w, Some(20), 40));
        // transitiv: 40 schreibt in 41 → 41 erbt {5, 20, 40}
        assert!(on_write(&mut w, Some(40), 41));
        assert_eq!(w[&41].meta.agent_chain, [5, 20, 40]);
        // Ein vom Nutzer gestarteter Agent ohne Kette markiert beim Schreiben nichts (wie bisher)
        let mut w = world(&[(1, false), (2, false)]);
        assert!(!on_write(&mut w, Some(1), 2));
        assert!(w[&2].meta.agent_chain.is_empty());
        assert!(!write_forbidden(&w, Some(2), 1), "zwei vom Nutzer gestartete Agenten dürfen sich gegenseitig schreiben");
    }

    #[test]
    fn a_chat_turn_started_by_an_agent_is_a_chain_member_for_its_writes() {
        // Agent 5 → Chat 10; die Runde schreibt per queue_prompt in die Claude-Sitzung 40 des Nutzers.
        let mut w = world(&[(5, false), (10, true), (40, false)]);
        agent_sends_chat(&mut w, 5, 10);
        assert!(w[&10].meta.agent_chain.is_empty(), "ein Agent ohne Kette markiert den Chat nicht dauerhaft");
        assert!(on_write(&mut w, Some(10), 40));
        assert_eq!(w[&40].meta.agent_chain, [5, 10]);
        assert!(write_forbidden(&w, Some(40), 5));
        // Ein Glied der Kette, das einem Chat schreibt, markiert ihn dauerhaft (auch nach einer Nutzer-Runde)
        let mut w = world(&[(5, false), (20, false), (11, true)]);
        on_create(&mut w, Some(5), 20);
        agent_sends_chat(&mut w, 20, 11);
        assert_eq!(w[&11].meta.agent_chain, [5, 20]);
        user_sends_chat(&mut w, 11);
        assert_eq!(ancestors(&w, Some(11)), [5, 20]);
        // Die Chat-Werkzeuge sehen dieselbe Kette (`App::chat_policy`)
        assert!(proxy_forbidden("send_text", &serde_json::json!({"session": 5, "text": "x"}), &ancestors(&w, Some(11))));
    }

    #[test]
    fn the_chain_survives_a_restart() {
        let mut w = world(&[(5, false), (20, false)]);
        on_create(&mut w, Some(5), 20);
        // Neustart: Metadaten aus state.json, neue Einträge (Prozesse neu gestartet, Ids gleich)
        let saved = serde_json::to_string(&w[&20].meta).unwrap();
        let meta: SessionMeta = serde_json::from_str(&saved).unwrap();
        let mut w = world(&[(5, false)]);
        w.insert(20, Entry::new(meta));
        assert!(write_forbidden(&w, Some(20), 5));
        assert!(!write_forbidden(&w, Some(5), 20));
    }

    #[test]
    fn an_agent_created_chat_cannot_write_into_its_agent_even_in_a_user_turn() {
        // Agent 5 legt Chat 12 an; der Nutzer schreibt selbst im Chat (keine Kette der Runde) – die Werkzeuge des
        // Modells dürfen trotzdem nicht in 5 schreiben, auch nicht per run_command/send_keys/queue_prompt/close/focus.
        let mut w = world(&[(5, false), (12, true), (30, false)]);
        on_create(&mut w, Some(5), 12);
        user_sends_chat(&mut w, 12);
        let origins = ancestors(&w, Some(12));
        assert_eq!(origins, [5]);
        for (tool, args) in [
            ("send_text", serde_json::json!({"session": 5, "text": "x"})),
            ("run_command", serde_json::json!({"session": 5, "command": "dir"})),
            ("send_keys", serde_json::json!({"session": 5, "keys": ["enter"]})),
            ("queue_prompt", serde_json::json!({"session": 5, "prompt": "x"})),
            ("close_session", serde_json::json!({"session": 5})),
            ("focus_session", serde_json::json!({"session": 5})),
        ] {
            assert!(proxy_forbidden(tool, &args, &origins), "{tool}");
        }
        assert!(!proxy_forbidden("send_text", &serde_json::json!({"session": 30, "text": "x"}), &origins));
        // Bei der Ausführung (mcp_target, für jeden Aufrufer, auch bei „Agenten dürfen fremde Sitzungen schließen“)
        assert!(write_forbidden(&w, Some(12), 5));
    }

    #[test]
    fn closed_sessions_are_pruned_from_every_chain() {
        let mut w = world(&[(5, false), (20, false), (21, true)]);
        on_create(&mut w, Some(5), 20);
        on_create(&mut w, Some(20), 21);
        w.get_mut(&21).unwrap().chat.as_mut().unwrap().origin_sessions = vec![5, 99];
        assert!(!prune(&mut w), "alle noch da");
        w.remove(&5);
        assert!(prune(&mut w));
        assert_eq!(w[&20].meta.agent_chain, Vec::<u64>::new());
        assert_eq!(w[&21].meta.agent_chain, [20]);
        assert!(w[&21].chat.as_ref().unwrap().origin_sessions.is_empty());
        assert!(!prune(&mut w));
        // Laden: eine Kette aus state.json mit Ids, die es nicht mehr gibt
        let meta: SessionMeta = serde_json::from_str(r#"{"id":40,"agent_chain":[3,20,77]}"#).unwrap();
        w.insert(40, Entry::new(meta));
        assert!(prune(&mut w));
        assert_eq!(w[&40].meta.agent_chain, [20]);
    }

    #[test]
    fn the_user_can_reset_a_chain() {
        let mut w = world(&[(5, false), (20, false)]);
        w.get_mut(&5).unwrap().meta.name = "Agent A".into();
        w.get_mut(&5).unwrap().meta.custom_name = true;
        on_create(&mut w, Some(5), 20);
        assert!(hint(&w, 20).is_some_and(|h| h.ends_with(": Agent A")), "{:?}", hint(&w, 20));
        assert_eq!(hint(&w, 5), None, "unmarkiert: kein Hinweis");
        assert!(reset(&mut w, 20));
        assert!(w[&20].meta.agent_chain.is_empty() && hint(&w, 20).is_none());
        assert!(!write_forbidden(&w, Some(20), 5));
        assert!(!reset(&mut w, 20), "nichts mehr zurückzusetzen");
        assert!(!reset(&mut w, 99));
    }

    #[test]
    fn a_user_started_agent_is_unaffected() {
        let w = world(&[(1, false), (2, true), (3, false)]);
        for c in [Some(1), Some(2), None] {
            assert!(ancestors(&w, c).is_empty());
            for t in [1, 2, 3] {
                assert!(!write_forbidden(&w, c, t));
            }
        }
        let (code, msg) = chain_error();
        assert_eq!(code, "forbidden");
        assert!(!msg.is_empty());
    }
}
