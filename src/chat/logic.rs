//! Reine Funktionen auf der Nachrichtenliste eines Chats (ohne Oberfläche, ohne Backend).

use super::store::{ChatMsg, Role};

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

/// Alles nach der letzten Nutzer-Nachricht entfernen (Antworten, Werkzeugergebnisse). `false` ohne Nutzer-Nachricht.
pub fn truncate_after_last_user(msgs: &mut Vec<ChatMsg>) -> bool {
    match last_user(msgs) {
        Some(i) => {
            msgs.truncate(i + 1);
            true
        }
        None => false,
    }
}

/// Letzte Nutzer-Nachricht durch `text` ersetzen und die Antworten danach verwerfen. `false` ohne Nutzer-Nachricht.
pub fn apply_edit(msgs: &mut Vec<ChatMsg>, text: &str) -> bool {
    let Some(i) = last_user(msgs) else { return false };
    msgs.truncate(i + 1);
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
    }
    out
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
