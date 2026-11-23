//! Reine Funktionen auf der Nachrichtenliste eines Chats (ohne Oberfläche, ohne Backend).

use super::store::{ChatMsg, Role, ToolCall, ToolStatus};

/// Geschätzte Tokens einer Nachricht (wie `llm::tokens::message_tokens` für die gesendete Fassung).
pub fn msg_tokens(m: &ChatMsg) -> usize {
    let calls: usize = m.tool_calls.iter().map(|c| crate::llm::tokens::estimate_tokens(&c.name) + crate::llm::tokens::estimate_tokens(&c.arguments.to_string()) + 3).sum();
    4 + crate::llm::tokens::estimate_tokens(&m.content) + calls
}

/// Ergebnis-Nachricht zu einem Werkzeugaufruf.
pub fn tool_result(call: &ToolCall, status: ToolStatus, content: String) -> ChatMsg {
    let mut m = ChatMsg::new(Role::Tool, content);
    m.tool_call_id = Some(call.id.clone());
    m.tool_name = Some(call.name.clone());
    m.status = Some(status);
    m
}

/// Werkzeugaufrufe ohne Ergebnis bekommen eines (`result`, z. B. „unterbrochen“), direkt hinter die vorhandenen
/// Ergebnisse ihrer Antwort. So bleibt der Verlauf für das Modell stimmig (zu jedem Aufruf genau ein Ergebnis), auch
/// nach einem Absturz oder Stopp mitten in einer Werkzeug-Runde. Fehlgeschlagene Antworten werden nicht gesendet und
/// bleiben unberührt. Gibt die Zahl der ergänzten Ergebnisse zurück.
pub fn close_orphans(msgs: &mut Vec<ChatMsg>, result: impl Fn() -> (ToolStatus, String)) -> usize {
    let mut added = 0;
    let mut i = 0;
    while i < msgs.len() {
        if msgs[i].role != Role::Assistant || msgs[i].tool_calls.is_empty() || msgs[i].error.is_some() {
            i += 1;
            continue;
        }
        let calls = msgs[i].tool_calls.clone();
        let mut j = i + 1;
        let mut answered = std::collections::HashSet::new();
        while j < msgs.len() && msgs[j].role == Role::Tool {
            if let Some(id) = &msgs[j].tool_call_id {
                answered.insert(id.clone());
            }
            j += 1;
        }
        for c in calls.iter().filter(|c| !answered.contains(&c.id)) {
            let (st, content) = result();
            msgs.insert(j, tool_result(c, st, content));
            j += 1;
            added += 1;
        }
        i = j;
    }
    added
}

/// Gehört die Nachricht in eine Anfrage an das Modell? Fehlgeschlagene Antworten nicht.
pub fn sendable(m: &ChatMsg) -> bool {
    !(m.role == Role::Assistant && m.error.is_some())
}

/// Index der letzten Nachricht des Nutzers.
pub fn last_user(msgs: &[ChatMsg]) -> Option<usize> {
    msgs.iter().rposition(|m| m.role == Role::User)
}

/// Kann die letzte Antwort neu erzeugt werden? Dafür muss nach der letzten Nutzer-Nachricht etwas stehen.
pub fn can_regenerate(msgs: &[ChatMsg]) -> bool {
    last_user(msgs).is_some_and(|i| i + 1 < msgs.len())
}

/// Alles nach Index `i` entfernen – außer Nachrichten von Paneldeck (#355, z. B. „Hintergrund-Befehl beendet“): die
/// bleiben in ihrer Reihenfolge stehen (sie berichten von Dingen, die wirklich geschehen sind).
fn truncate_keep_events(msgs: &mut Vec<ChatMsg>, i: usize) {
    let kept: Vec<ChatMsg> = msgs.drain(i + 1..).filter(|m| m.event.is_some()).collect();
    msgs.extend(kept);
}

/// Alles nach der letzten Nutzer-Nachricht entfernen (Antworten, Werkzeugergebnisse; Nachrichten von Paneldeck bleiben).
/// `false` ohne Nutzer-Nachricht.
pub fn truncate_after_last_user(msgs: &mut Vec<ChatMsg>) -> bool {
    match last_user(msgs) {
        Some(i) => {
            truncate_keep_events(msgs, i);
            true
        }
        None => false,
    }
}

/// Letzte Nutzer-Nachricht durch `text` ersetzen und die Antworten danach verwerfen (Nachrichten von Paneldeck bleiben).
/// `false` ohne Nutzer-Nachricht.
pub fn apply_edit(msgs: &mut Vec<ChatMsg>, text: &str) -> bool {
    let Some(i) = last_user(msgs) else { return false };
    truncate_keep_events(msgs, i);
    msgs[i].content = text.to_string();
    msgs[i].ts = super::now_secs();
    true
}

/// `context_start` passend zur (gekürzten) Liste: ein Index hinter dem Ende gilt nicht mehr.
pub fn clamp_context_start(cs: Option<usize>, len: usize) -> Option<usize> {
    cs.filter(|c| *c > 0 && *c < len)
}

/// Verlauf als Markdown (Export „Verlauf als Datei speichern“).
pub fn to_markdown(title: &str, model: &str, msgs: &[ChatMsg], lang: crate::state::Lang) -> String {
    let mut out = format!("# {title}\n\n{}: `{model}`\n", lang.t("Modell", "Model"));
    for m in msgs {
        let head = match m.role {
            Role::System => lang.t("System", "System").to_string(),
            Role::User => lang.t("Du", "You").to_string(),
            Role::Assistant => model.to_string(),
            Role::Tool => format!("{} `{}`", lang.t("Werkzeug", "Tool"), m.tool_name.as_deref().unwrap_or("?")),
        };
        out += &format!("\n## {head}\n\n");
        if m.role == Role::Tool {
            out += &format!("```\n{}\n```\n", m.content.trim_end());
        } else if !m.content.is_empty() {
            out += m.content.trim_end();
            out.push('\n');
        }
        for c in &m.tool_calls {
            out += &format!("\n- {} `{}` `{}`\n", lang.t("Werkzeugaufruf", "Tool call"), c.name, c.arguments);
        }
        if let Some(e) = &m.error {
            out += &format!("\n> **{}:** {e}\n", lang.t("Fehler", "Error"));
        }
        if let Some(n) = &m.note {
            out += &format!("\n> {n}\n");
        }
    }
    out
}

#[cfg(test)]
mod event_tests {
    use super::*;

    /// #355 Review: Neu erzeugen und Bearbeiten werfen Nachrichten von Paneldeck nicht weg.
    #[test]
    fn regenerate_and_edit_keep_paneldeck_messages() {
        let ev = || {
            let mut m = ChatMsg::new(Role::System, "bg1 finished");
            m.event = Some(super::super::store::ChatEvent { kind: "background_command".into(), id: "bg1".into(), ..Default::default() });
            m
        };
        let mut msgs = vec![ChatMsg::new(Role::User, "q"), ChatMsg::new(Role::Assistant, "a"), ev(), ChatMsg::new(Role::Assistant, "b")];
        assert!(truncate_after_last_user(&mut msgs));
        assert_eq!(msgs.iter().map(|m| (m.role, m.event.is_some())).collect::<Vec<_>>(), [(Role::User, false), (Role::System, true)]);
        msgs.push(ChatMsg::new(Role::Assistant, "c"));
        assert!(apply_edit(&mut msgs, "q2"));
        assert_eq!((msgs[0].content.as_str(), msgs.len(), msgs[1].event.is_some()), ("q2", 2, true));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conv() -> Vec<ChatMsg> {
        vec![
            ChatMsg::new(Role::System, "sys"),
            ChatMsg::new(Role::User, "q1"),
            ChatMsg::new(Role::Assistant, "a1"),
            ChatMsg::new(Role::User, "q2"),
            ChatMsg::new(Role::Assistant, "a2"),
            ChatMsg::new(Role::Tool, "t"),
            ChatMsg::new(Role::Assistant, "a2b"),
        ]
    }

    #[test]
    fn regenerate_drops_everything_after_the_last_question() {
        let mut v = conv();
        assert!(can_regenerate(&v));
        assert!(truncate_after_last_user(&mut v));
        assert_eq!(v.len(), 4);
        assert_eq!(v.last().unwrap().content, "q2");
        assert!(!can_regenerate(&v), "nichts mehr zum Neuerzeugen");
        let mut empty = vec![ChatMsg::new(Role::System, "s")];
        assert!(!truncate_after_last_user(&mut empty));
        assert_eq!(empty.len(), 1);
    }

    #[test]
    fn edit_replaces_the_last_question_and_drops_answers() {
        let mut v = conv();
        assert!(apply_edit(&mut v, "q2 edited"));
        assert_eq!(v.iter().map(|m| m.content.as_str()).collect::<Vec<_>>(), ["sys", "q1", "a1", "q2 edited"]);
        let mut none: Vec<ChatMsg> = Vec::new();
        assert!(!apply_edit(&mut none, "x"));
    }

    #[test]
    fn failed_answers_are_not_sent() {
        let mut m = ChatMsg::new(Role::Assistant, "");
        assert!(sendable(&m));
        m.error = Some("x".into());
        assert!(!sendable(&m));
        assert!(sendable(&ChatMsg::new(Role::User, "u")));
    }

    #[test]
    fn context_start_is_clamped() {
        assert_eq!(clamp_context_start(Some(3), 5), Some(3));
        assert_eq!(clamp_context_start(Some(5), 5), None);
        assert_eq!(clamp_context_start(Some(0), 5), None);
        assert_eq!(clamp_context_start(None, 5), None);
    }

    #[test]
    fn orphan_tool_calls_get_a_result_in_place() {
        let call = |id: &str| ToolCall { id: id.into(), name: "list_sessions".into(), arguments: serde_json::json!({}) };
        let mut a = ChatMsg::new(Role::Assistant, "");
        a.tool_calls = vec![call("a"), call("b"), call("c")];
        let mut failed = ChatMsg::new(Role::Assistant, "");
        failed.tool_calls = vec![call("x")];
        failed.error = Some("boom".into());
        let mut v = vec![ChatMsg::new(Role::User, "q"), a, tool_result(&call("b"), ToolStatus::Ok, "{}".into()), ChatMsg::new(Role::User, "next"), failed];
        let n = close_orphans(&mut v, || (ToolStatus::Error, "interrupted".into()));
        assert_eq!(n, 2);
        let ids: Vec<_> = v.iter().map(|m| m.tool_call_id.clone().unwrap_or_default()).collect();
        assert_eq!(ids, ["", "", "b", "a", "c", "", ""], "Ergebnisse direkt hinter die Antwort, vor die nächste Frage");
        assert_eq!(v[3].status, Some(ToolStatus::Error));
        assert_eq!(v[3].tool_name.as_deref(), Some("list_sessions"));
        assert_eq!(close_orphans(&mut v, || (ToolStatus::Error, String::new())), 0, "zweiter Lauf ändert nichts");
    }

    #[test]
    fn markdown_export_has_all_parts() {
        let mut v = conv();
        v[4].tool_calls.push(super::super::store::ToolCall { id: "1".into(), name: "list_sessions".into(), arguments: serde_json::json!({}) });
        v[5].tool_name = Some("list_sessions".into());
        v[6].error = Some("boom".into());
        let md = to_markdown("Chat", "llama3", &v, crate::state::Lang::En);
        for needle in ["# Chat", "`llama3`", "## You", "q2", "## llama3", "Tool `list_sessions`", "Tool call `list_sessions`", "**Error:** boom"] {
            assert!(md.contains(needle), "{needle} fehlt in:\n{md}");
        }
    }
}
